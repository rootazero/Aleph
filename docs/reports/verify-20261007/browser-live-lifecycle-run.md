# Browser-live lifecycle run (#55): real target destroy + real TTL expiry

Date: 2026-10-11 (Windows, single machine). Only the LLM is mocked
(`mock-llm.js` on 127.0.0.1:18791, PID 2664, left running). Real Gateway,
real Chrome/CDP, real WS, real WASM DOM, real `browser_tabs` tool. No Rust edit,
no rebuild, no server restart, no mock clock, no request TTL, no synthetic grant.

## Runtime snapshot (not restarted)

* Gateway PID 12704, `127.0.0.1:18790` LISTENING, `GET /health` 200 before and after.
* Exe `target/debug/aleph-server.exe` of this worktree, mtime 2026-10-11 21:22:37
  (+08:00), process start 21:22:50, sha256 `5f827ee879bd10220365f69b196ffcb1eb0881bd2fed7cfeac5f48274a3a0034`
  (as printed by `sha256sum`).
* Source: HEAD `34a26aece` with a large dirty working tree (151 status entries);
  the binary is the build of that dirty tree at the mtime above. TypeScript-only
  changes here do not require a rebuild.
* Config: `.analysis/browser-live-runtime/config.toml`.

## Files

* `qa/browser_live/lifecycle-driver.cjs` (new, reusable driver),
  `qa/browser_live/run.sh` (+2 cases, fail closed on URL + SIGNAL_DIR),
  `qa/browser_live/tests/viewer.spec.ts` (+`target-destroy:`, `ttl-expiry:`,
  `expectInterrupted(page, timeout)`), `qa/browser_live/check_docs.py`
  (11 selectors), `qa/browser_live/README.md` (new) and the selector lists in
  `docs/reference/DEVELOPMENT.md`, `qa/README.md`, `docs/reference/BROWSER_LIVE_VIEWER.md`.
* Logs: `.analysis/browser-live-runtime/lifecycle/{target-destroy,ttl-expiry}.log`
  (+ `ttl-expiry.rc`). They contain only sanitized JSON; no URL, token or frame.

## Static checks

* `py qa/browser_live/check_docs.py --self-test` OK; `py qa/browser_live/check_docs.py` OK (7 files).
* `npx playwright test --config=qa/browser_live/playwright.config.ts --list`: 11 tests.
* `run.sh --case target-destroy|ttl-expiry` without env: rc 1 fail closed
  (`ALEPH_BROWSER_LIVE_VIEWER_URL must be ...`).
* `--probe`: `browser_tabs list` ids contain the resource `target_id` (10 tabs in the shared profile).

## Case 1 — target-destroy: PASS (case exit 0, driver exit 0)

| item | value |
|---|---|
| run.sh / Playwright exit (`pw_exit`) | 0 (`1 passed`, 1.5 s) |
| initial frame | real JPEG frame seen, `target-destroy.ready` written at 1791772326115 |
| correlation | `browser_tabs list`: 11 tabs, exactly 1 with `id === resource target_id` |
| action | `browser_tabs close {profile:'default', action:{close:{tab_id}}}` at 1791772326360, `success: true` |
| terminal event | `browser.live.closed`, `reason: "target_closed"` (at_ms 1791772326384), resource_id and binding target equal the issued resource; 24 ms after the close request |
| DOM | frame count 0 and closed status visible (generic closed text) |
| other closed events | 0 |

## Case 2 — ttl-expiry: PASS (case exit 0, driver exit 0)

Real clock, fresh grant, viewer open the whole time (Playwright reports 10.0 min).

| item | value (ms since epoch / durations) |
|---|---|
| `pw_exit` | 0 (`1 passed (10.0m)`) |
| resource event received | 1791772333630 |
| resource `expires_at_ms` | 1791772933629 (= event + 599 999 ms) |
| grant `expires_at_ms` | 1791772933629 (identical to the resource; grant request took 1 ms) |
| initial frame | `ttl-expiry.ready` at 1791772336774; remaining resource TTL at that moment 596 646 ms |
| terminal event | `browser.live.closed`, `reason: "expired"`, at_ms 1791772934563 |
| event minus resource/grant `expires_at_ms` | +934 ms (1 s sweeper tick) |
| event minus resource event | 600 933 ms |
| DOM | frame count 0 and closed status visible |
| other closed events seen | 1 (not attributed; most likely expiry of the earlier ungranted `--probe` resource, whose 600 s elapsed during this run — **unverified**) |

The fixed TTL is really 600 s measured from resource publication (599 999 ms to
`expires_at_ms`); the grant was issued 0.26 s after the resource, so the grant did
not shorten the effective expiry (min grant/resource = identical value).

## Cleanup

Each case used its own `os.tmpdir()/aleph-live-<case>-*` private marker directory
(removed by the driver; 0 left) and a Playwright-owned headless Chrome with a
temporary profile (exited with the test). No `rundll32`/default browser, no
foreign PID touched. Gateway 12704 and fixture 2664 still LISTENING; the target-destroy
test closed only its own bound tab (the other pre-existing tabs of the shared
`default` profile were left alone). Probe-created resources/tabs from earlier
runs are not cleaned up by this run.

## Not verified / negatives

* Gateway shutdown lifecycle (`service_exited`/`engine_exited`/`run_ended`).
* Panel in-app launch click, TUI launcher, Native IPC (unverified in this run only; later narrow QA-owned-root + fake Gateway + in-memory backend evidence: [native-ipc-qa-audit.md](native-ipc-qa-audit.md), not production OS keyring / protected pairing / full Panel), protected pairing, managed-companion
  (still BLOCKED), macOS/Linux, Obscura eventless/subframe.
* DOM text cannot distinguish reasons (the viewer shows a generic closed status);
  the discriminator is the driver-observed event reason.
* Single run of each case; no repeat/flake statistics. No Cargo/Rust checks were run.
