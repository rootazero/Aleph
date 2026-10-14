# qa/browser_live — real browser-live viewer QA

Real Playwright/Chromium runner for the Gateway-owned browser live viewer
(`/viewer/browser`). There is no synthetic resource, fake grant, mock clock or
fake IPC: every case needs a **Gateway-issued viewer URL** passed through the
environment (never argv / config / report). See
`docs/reference/BROWSER_LIVE_VIEWER.md`.

```
qa/browser_live/run.sh --case <selector>
```

Eleven selectors: `stream`, `control-all-entries`, `standalone-same-target`,
`binding-switch`, `managed-companion` (BLOCKED: no managed producer on Windows),
`grant-boundary`, `reap`, `revoke-live`, `navigate-fence`, `target-destroy`,
`ttl-expiry`. Missing environment variables fail closed.

## Interruption cases (external driver)

`revoke-live`, `navigate-fence`, `target-destroy` and `ttl-expiry` need
`ALEPH_BROWSER_LIVE_VIEWER_URL` plus `ALEPH_BROWSER_LIVE_SIGNAL_DIR`. The viewer
test first asserts a real JPEG frame, then writes `<case>.ready` into the signal
directory; an **external driver** performs / observes the real Gateway-side
effect. The viewer test never holds a Gateway credential.

`lifecycle-driver.cjs` drives the two lifecycle cases end to end (device token is
read from `ALEPH_BROWSER_LIVE_AUTH_FILE`, default
`.analysis/browser-live-runtime/device-token.json`, and only used in memory):

```
node qa/browser_live/lifecycle-driver.cjs target-destroy
node qa/browser_live/lifecycle-driver.cjs ttl-expiry     # real ~600 s wait
```

* `target-destroy`: proves `browser_tabs list` has exactly one tab whose `id`
  equals the resource-bound `target_id` (aborts with exit 5 otherwise), closes
  that tab via `browser_tabs close {tab_id}` and requires the terminal event
  `browser.live.closed` with `reason = "target_closed"` plus the viewer DOM
  losing its frame and showing the closed status.
* `ttl-expiry`: issues a fresh grant and keeps the viewer open until the fixed
  600 s TTL really elapses (no server TTL change; Playwright test timeout 780 s);
  requires `browser.live.closed` with `reason = "expired"` plus the DOM
  interrupted. Reports the sanitized numeric resource / grant `expires_at_ms`.

The viewer DOM shows one generic closed status for every interruption, so the
terminal event `reason` is the discriminator, not the DOM text.

Driver exit codes: 0 ok, 2 setup, 3 no resource, 4 decide/grant, 5 target
correlation not proven, 6 close failed, 7 wrong/missing terminal event, 8 no
initial frame, 10 Playwright (`pw_exit`) failed, 9 fatal. Optional
`ALEPH_BROWSER_LIVE_CHROME_PATH` selects the owned headless Chrome.
