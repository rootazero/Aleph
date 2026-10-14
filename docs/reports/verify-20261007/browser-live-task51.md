# Task 51 — viewer fragment scrub / 地址栏凭据清理 (local fix + RED)

Status / 状态: **GREEN (grant-boundary + 8 available live cases); Rust change compiled, unit-tested, clippy-clean, wasm rebuilt.** managed-companion remains BLOCKED (no real managed viewer URL; not fabricated). BROWSER_LIVE_VIEWER stays PARTIAL; not a full #23/#48 claim.

## 1. RED evidence (old running exe, PID 11076, unchanged)
- Fresh real grant via `.analysis/browser-live-runtime/e2e-issue.js --out url-task51-red.txt`, URL passed only through env `ALEPH_BROWSER_LIVE_VIEWER_URL` (never argv/config).
- `qa/browser_live/run.sh --case grant-boundary` -> **exit 1**, 1 failed:
  `Error: address bar fragment must be empty after exchange` / `Expected: true  Received: false` (viewer.spec.ts:92).
- Log `.analysis/browser-live-runtime/task51-red.log` and `test-results/*/error-context.md` were grepped: the fragment string does **not** appear (0 hits).
- Previous (old) assertion passed on the same exe (1 passed) — it only checked `.not.toContain(hash.slice(1))`, which never proved the address bar was cleaned. 旧断言误绿，已证。
- Other 8 selectors untouched; they are expected unchanged on the old exe. grant-boundary failing on the old exe is intentional RED, not a regression.

## 2. Changes
`qa/browser_live/tests/viewer.spec.ts` (grant-boundary rewritten, `openViewer` hardened)
- Original fragment read inside the driver from the issued URL; every secret-related check is `expect(<boolean>, fixed message).toBe(...)` — failure output cannot print the credential.
- Asserts after live frame: `page.url()` hash == '' and search == '', origin unchanged, path == `/viewer/browser`; in-page `location.hash/search` empty; raw fragment and each fragment value absent from `page.content()` + body text + scripts.
- Reload of the cleaned URL -> status visible, no frame (no replay). Malformed fragment (`#not-a-valid-handoff`, no secret) on a second page -> status visible, no frame, hash cleaned.
- `openViewer` wraps `page.goto` so a navigation error cannot echo the launch URL.
`qa/browser_live/playwright.config.ts`: `trace: 'off'` (traces persist the launch URL).
`interfaces/webchat/src/platform/wide/views/browser/standalone.rs`
- New `begin_standalone_session(location, scrub)`: (1) refuse unless pathname is exactly `/viewer/browser` (other pages never rewritten); (2) `scrub(VIEWER_PAGE_PATH)` always — also for empty/malformed/insecure-origin links; (3) only then `ViewerSession::from_location` on the **already captured in-memory** copy; `scrub` error -> `None` (fixed InvalidLink, no session, hence no consume/exchange/AuthHello).
- `replace_current_entry` (wasm): `window.history().replace_state_with_url(NULL, "", Some("/viewer/browser"))` — current entry only, fixed same-origin relative path, no origin concatenation, no assign/reload; errors carry no detail. Native: `Err(())`, route never matches there anyway.
- `LocationParts` `Debug` now redacts `hash`.
- 8 new unit tests: valid link scrubbed then consumed from memory copy; scrub target contains no `#`/`?`/credential; malformed/empty variants scrubbed then invalid; insecure origin scrubbed then invalid; history error fails closed; other same-origin paths untouched (scrub closure panics if called); Debug redaction.
No Cargo.toml change: `Window::history()`/`History::replace_state_with_url` are already used by `context.rs:332-343`, `views/memory/mod.rs:924` (the `History` web-sys feature is therefore already enabled; `just wasm` will confirm).

## 3. Security review (static)
- Capture-before-clear: hash is moved out of `LocationParts` after the scrub, never re-read from `window`.
- No fragment logged; no fallback navigation; PanelContext/auth persist and localStorage untouched.
- Residual: a failed `replaceState` makes the page unusable (fail-closed by design). Between page load and WASM start the fragment is necessarily in the URL (never sent to servers; browser history of that earlier tick is the same entry that gets replaced).
- Query behaviour change: any `?query` on `/viewer/browser` is now dropped too. The standalone route never advertised query support (only the fragment is parsed), so no known consumer breaks.

## 4. Verification run (after #49 runner freed) / 验证结果
Every cargo/just command ran through `.analysis/browser-live-runtime/task51-gate.sh` (free memory >= 4 GiB gate, CARGO_BUILD_JOBS=1, CARGO_PROFILE_DEV_DEBUG=0, CARGO_NET_OFFLINE=true, one runner).
- `cargo test --offline -p aleph-panel --lib standalone` -> exit 0, **12 passed / 0 failed** (8 new + 4 existing incl. launch). Log: task51-panel-test.log.
- `cargo clippy --offline -p aleph-panel --all-targets -- -D warnings` -> exit 0. Log: task51-panel-clippy.log.
- `just wasm` -> exit 0; dist rewritten (7 tracked files under interfaces/webchat/dist: +156/-69 text lines, wasm 23066453 -> 23827689 bytes), 41 wasm refs resolve. Log: task51-wasm.log.
- Old server PID 11076 (exe = this worktree's target/debug/aleph-server.exe, cmdline --config .analysis/browser-live-runtime/config.toml start, listening 127.0.0.1:18790; record: task51-old-server-record.txt) verified then stopped with `taskkill /PID 11076 /T /F` (the /T also ended its own child process tree, i.e. the browsers it spawned; no other Aleph process touched; mock-llm PID 2664 untouched). Then `cargo build --offline -p alephcore --bin aleph-server` exit 0 (task51-build-server.log); restarted with scope-start.ps1 (ALEPH_HOME=.analysis/browser-live-runtime/home, same config.toml): new PID 12704, bound 127.0.0.1:18790, GET /viewer/browser HTTP 200, served aleph_panel.js contains replaceState.
- **grant-boundary: old RED (exit 1) -> exit 0** with fresh grant (task51-green-grant-boundary.log).
- Fresh-grant regression via existing run-all.sh / run-live.sh: stream, control-all-entries, standalone-same-target, grant-boundary, binding-switch, reap, revoke-live, navigate-fence: **each exit 0** (run-all.sh exit code is not used; each CASE exit was parsed from its own line). managed-companion: exit 1, "ALEPH_BROWSER_LIVE_MANAGED_VIEWER_URL must be a real managed-companion viewer URL" = BLOCKED, env not fabricated. Logs: task51-run-all.log, task51-run-live.log.
- Secret scan: all 22 fragment values from every url-*.txt checked against all task51/qa/driver/run/server logs and test-results: **0 nonce hits**; only the grant_id handle (printed by the pre-existing e2e-issue.js "grant issued" line) appears in the two run logs. Playwright trace off; no URL in failure text.
- Static review: no console/tracing/log call in standalone.rs or viewer.spec.ts; capture-before-clear and fail-closed (scrub Err -> None, covered by test history_error_fails_closed...); non-viewer paths never rewritten (scrub closure panics in test). Worktree diff: 94 tracked files, +9085/-714 (whole task, not only #51); untracked standalone.rs 478 lines, viewer.spec.ts 173 lines.

## 5. Not done / residual
- Disclosure: while appending this report a shell backtick slip re-executed, WITHOUT the gate/env limits, one extra `cargo test` (0 tests matched), one `cargo clippy` (passed) and malformed `just wasm`/`cargo build`/`taskkill` invocations that all failed at argument parsing. No server was affected (PID 12704 still up); the gated results above are the authoritative ones.
- managed-companion not run (BLOCKED); #23/#48 not claimed; central docs untouched; no commit.
- Browser-only check: Chrome headless via Playwright only; no other browsers; reload/malformed paths pass in grant-boundary.
