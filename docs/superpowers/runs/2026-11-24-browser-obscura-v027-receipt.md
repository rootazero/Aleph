# Receipt — obscura v0.2.7 upgrade / obscura v0.2.7 升级回执

> Evidence layer (时点记录). Design: `docs/superpowers/specs/2026-11-24-browser-obscura-v027-upgrade-design.md`;
> plan: `docs/superpowers/plans/2026-11-24-browser-obscura-v027-upgrade.md`.
> Branch `browser-obscura-v027` from `main` 99090adce. Host: arm64 macOS, Chrome for the chromium side.
> If this receipt and the code disagree, the code wins.

## What changed / 改了什么

| Commit | Change |
|---|---|
| 3d8734583 | `aleph-cdp` `BoxModel.width/height` accept fractional CSS px (obscura serializes `coord_value` floats; the `i64` decode failed the whole reply → no rect) — rounded, non-finite rejected |
| 44ff99284 | `qa/browser_dual` caps measures `drag` and `js_dialogs` **by effect** with the verbs Aleph actually sends; QA derives the obscura tag from `src/runtimes/specs.rs` |
| c6f2d703d | Pin `v0.2.7`; effect-probe document nonce (`window.__alephProbeDoc`); obscura fabricated fallback box (`[8,8,108,8,108,28,8,28]`, 100×20) read as no box; visibility no longer cascaded by Aleph (v0.2.7 resolves it); dated-claim stamp → v0.2.7 sha256 `d09acc26…`; capability rows `drag` / `effect_probe` → Supported |
| eab3b17f1 | rustfmt |
| 0cb486484 | Route obscura's page-started main-frame `Page.frameNavigated` by frame id when it arrives on the session Aleph did not attach (v0.2.7 has two sessions per page) |
| 332cca7a3 | `drive.py` expectations re-dated to 0.2.7 (`block_ref` by name; click-driven nav turns the loader over; inline link has a box) |
| 21d362a2f | `provision`: read the dead-mirror failure from the install job's outcome; `ALEPH_GATEWAY_TOOLS_ALLOW=bash` on that stage's isolated server only |
| b636e01b0 | v0.2.2-era comments re-measured on v0.2.7 carry the new reading; un-re-measured claims keep their dated v0.2.2 wording |
| a950296b9 | Route a `Fetch.requestPaused` that arrives on the page's other session to the slot keyed by its `frameId` (`on_fetch`, `src/browser/live/mod.rs`); caps measures `network_interception` by effect, raw and through `browser_network{mock_add}`; obscura row → Supported |

## Capability table, final / 能力表终值 (obscura, measured 2026-11-25, v0.2.7)

| Row | Value | Reading |
|---|---|---|
| js_dialogs | Unsupported | no `javascriptDialogOpening` for alert/confirm/prompt (only `beforeunload`); `handleJavaScriptDialog` -32601 |
| drag | **Supported** (was Unsupported) | six-event mouse gesture → `dropped:mouseup`; **no HTML5 `dragstart`/`drop`** — a drop-only target does not react |
| file_upload | Unsupported | still gated by `--allow-file-access`, which Aleph never passes |
| pdf / insert_text / ref_precheck | Supported | unchanged |
| effect_probe | **Supported** (was Unsupported) | readback after a self-navigating click still clean null, but the nonce is gone on the new doc → Skipped, not a false miss |
| network_interception | **Supported** (was Unsupported) | a page `fetch()` of a URL the server lacks resolves to the mock, raw (fulfill on our session) and through `browser_network{mock_add}`; depends on the frameId route for pauses on the other session |
| screencast | Supported | usable frames |
| error_events | Unsupported | async throw → no `Runtime.exceptionThrown` within 10 s |

caps stage: table == measurement on all 10 rows, `measured_on` == binary (`v0.2.7`).

## Other readings / 其他测量

- **Screenshot geometry:** `#geo` = [137, 211, 53, 29] in box model, client rect and screenshot pixels (image 1280 = CSS 1280) on both engines → no screenshot note added. One absolutely positioned box only; normal-flow layout not measured.
- **loaderId (⑯b):** the stale loader did not reproduce; the main-frame event carries the new loader. Read order `result.loader_id.or(outcome.loader_id)` kept.
- **Two sessions per page:** `<target>-session` (createTarget) and `<target>-session-<n>` (attachToTarget); page-started navigation lands on either. Before the fix, `click` failed 7 of 11 runs; after it, 6 of 6 on v0.2.7, 0 fails on v0.2.2.
- **Visibility:** v0.2.7 resolves inherited `visibility` (upstream f019ebe), so Aleph's own cascade of it was removed. An operator-pinned v0.2.2 loses that cascade (documented in `fetch_obscura.rs`).
- **`DOM.disable`:** v0.2.7 accepts it; v0.2.2 answers -32601. Box-model / resolveNode still work after it.
- **Fabricated / fractional boxes on real pages (2026-11-25, plan Task 4 Step 5):** `DOM.getBoxModel` on every element of the four QA pages (`caps.html` 11, `index.html` 21, `second.html` 5, `stall.html` 6): 0 fabricated fallback boxes, 0 no-box replies, and 3 elements on `index.html` with a fractional width or height. The fractional-size decode fix is therefore hit on ordinary pages; the fabricated-box guard is a backstop that none of these pages trigger.
- **`qa-events-probe.mjs` (3 runs, identical):** `Runtime.exceptionThrown` never arrived (the throwing callback ran); `Target.targetDestroyed` arrived for both `closeTarget` and `Page.close`. Verdict PARTIAL — matches `error_events: Unsupported`.
- **`fetch-probe.mjs`:** unmodified, it times out on its own `await Page.navigate` (v0.2.7 now holds the navigation, and obscura answers neither `Page.navigate` nor `Runtime.evaluate` while a pause is held — the probe was written for v0.2.2's non-holding pause). A scratch copy that awaits the navigate only after the fulfill, and that tolerates the evaluate timeout during the hold, measured 3 runs: enable ok, navigation pause arrives, holds, and the fulfill serves the mock body 3/3; the subresource (`fetch()`) pause arrived on our session 1/3. Listening on every session (6 runs): the subresource pause landed on `<target>-session` (the createTarget session) 3/6 and on ours 3/6, and fulfilling it on OUR session served the mock 6/6. So v0.2.7's Fetch pipeline works; what failed was the same two-sessions-per-page routing as the click fix, which a950296b9 closes (see below).
- **`network_interception` through Aleph (2026-11-25, after a950296b9):** caps 6 runs with the route, all PASS (43/0); the raw half saw the pause on the foreign session in 2 of 6 and on ours in 4 of 6, and its `frameId` named our target every time. Tool half: 3 `fetch()`es per run through `browser_network{mock_add}` all resolved to the rule's body. **Negative control:** the same build with the frameId route disabled by a temporary switch: 3 of 4 runs left every mocked `fetch()` pending (never answered), 1 of 4 passed (that process happened to report on Aleph's session). The route is what makes the row hold. The QA approval policy file gained `browser_network_mock: allow` for this check.

## Verification / 验证

- `cargo clippy -p alephcore -p aleph-cdp --all-targets -- -D warnings`: clean.
- `cargo test -p aleph-cdp`: 97 passed.
- `cargo test -p alephcore --lib -- browser runtimes builtin_tools::browser_tools`: 1355 passed, 4 failed. All four also fail on clean `main` 99090adce (`browser::live::issuer` ×2, `browser::live::owner_registry` ×1, `browser_tools::tabs::no_browser_tool_formats_a_raw_backend_error`) and are not from this branch.
- After a950296b9: clippy clean; the same filter 1357 passed, the same 4 failed; `qa/browser_dual` caps 43/0 (×3), click 26, switch 29, escape 14, open 19, snapshot 12, all 0 FAIL. Chromium mock routes (`qa/browser_managed`) were not rerun.
- `qa/browser_dual/run.sh` on v0.2.7, after b636e01b0:

| Stage | PASS | FAIL | Note |
|---|---|---|---|
| provision | 21 | 0 | Initial run at 21d362a2f. The later run below needed the anonymous GitHub API quota to reset. |
| open | 19 | 0 | |
| snapshot | 12 | 0 | |
| click | 26 | 0 | |
| stall | 17 | 0 | KNOWN GAP still present on 0.2.7: the snapshot path calls a busy engine "stopped" |
| reap | 13 | 0 | |
| caps | 34 | 0 | 43 / 0 after a950296b9 (×3), with network_interception tool-face checks |
| switch | 29 | 0 | |
| escape | 14 | 0 | |

### Follow-up coverage on macOS arm64 / macOS arm64 补测（2026-11-25）

| Stage | PASS | FAIL | Evidence / 证据 |
|---|---:|---:|---|
| caps | 51 × 4 runs | 0 | `browser_navigate` completes for a nonexistent fixture path; the resulting URL and top-level DOM contain the expected path and mock marker. 真实导航完成，顶层 DOM 渲染 mock 文档。 |
| caps (same runs / 同上) | — | — | A real same-origin iframe issues its own `fetch()` and posts back `200:MOCK-TOOL-7f3a`; the parent verifies the sender and origin. The fixture server received the iframe document and the post-clear control (404), but not the mocked iframe or top-level document requests in the retained HTTP log. iframe 自己发请求，父页校验消息来源并读回状态码和正文。 |
| provision | 21 | 0 | First attempt was blocked at preflight by unauthenticated GitHub HTTP 403 (exit 69); retry after quota reset passed with the unmodified installer, including binary verification and the unreachable-mirror check. 匿名配额恢复后，真实安装及坏镜像反证均通过。 |
| stall | 17 | 0 | 2 s timeout and recovery verified; the known busy-as-stopped snapshot message still reproduces. 再现已知提示文案问题。 |
| reap | 13 | 0 | Replacement server reaped both engine orphans and left the prefix-neighbour process alive. 两个引擎的孤儿进程均被清理，相邻进程保留。 |

## Independent review / 独立审查

A read-only branch review (openai-codex/gpt-6.1-sol, 2026-11-25) blocked the merge on one P1 and raised one P2. Both are fixed.

- **P1 — iframe read-back in the wrong realm.** The install runs `Runtime.callFunctionOn` on the target node, so its `window` is the node's realm. For a target in a same-renderer iframe or `srcdoc` frame, which `resolve_target` admits, that realm is the frame's own global. The read-back was a page-level `Runtime.evaluate`, so it read the main frame and found no nonce. Every iframe action was therefore reported `Skipped`, real misses included, and the frame-side listeners were never removed. Fix: `ArmedProbe { nonce, object_id }` in `src/browser/cdp_backend/actions.rs`; the read-back is now a `Runtime.callFunctionOn` on that same node. After a navigation the read-back through the old node was measured, not assumed: Chrome 158 main-frame link 3/3 and obscura v0.2.7 caps 3/3 → call error ("Cannot find context with specified id" on Chrome) → `Unknown` → `Skipped`; Chrome 158 `srcdoc` iframe link 3/3 → `click` from the old realm → `Verified` (correct). The one unsound answer — a clean `null` in a realm that kept the nonce, i.e. a false `EffectNotDelivered` — was not seen on either engine and is not guaranteed against; caps premise 3 now reads back through the installed node exactly as production does and refuses obscura's row on it (re-review P2, 2026-11-25). Test: `the_readback_runs_in_the_realm_the_probe_was_installed_in`. Real Chrome 158 check, a `srcdoc` iframe button probed by raw CDP: the old main-frame read gave `{otherDocument: true}` on both a hit and a miss; the read on the node gave `click` and `null`.
- **P2 — the geometry reading could not fail caps.** `qa/browser_dual/caps.py` now `led.check`s `geometry_agrees`, which pins the "no screenshot coordinate warning" conclusion.
- After the fixes: `cargo test -p alephcore --lib -- browser::cdp_backend` 186 passed; clippy clean; QA on v0.2.7: click 26, caps 35, switch 29, escape 14, open 19, snapshot 12, all with 0 FAIL. After the re-review's navigation change to premise 3: caps 36 PASS / 0 FAIL, three runs.

Review of a950296b9 + 3cbc0308e (openai-codex/gpt-6.1-sol, read-only, 2026-11-25): OK with notes. No misrouting path found: a session match comes first, and the fallback only selects an already-registered slot keyed by the full target id. No navigation deadlock: `navigate.rs` starts the intercept worker before it awaits `Page.navigate`, and the worker answers on its own. One P2: caps' `mock_clear` check only read the reply. Fixed: it now asserts that `mock_list` is empty and that a fresh `fetch()` of the same URL reaches the server (404). caps 45 / 0, ×2.

## Deliberately not done / 刻意不做

- No automatic engine switching; the model still decides (R7/R10).
- No upstream obscura patches; no `--allow-file-access`.
- `fetch_obscura.rs` stays: v0.2.7 `DOMSnapshot` still fabricates geometry (`y = i*18`).
- The Aleph Fetch follow-up covers same-origin iframe subresources and top-level navigation documents; it does not cover cross-origin iframe requests, iframe document navigation, or Chromium mock routes. 未覆盖跨源 iframe 请求、iframe 文档导航或 Chromium mock 路由。
- No Linux / Windows run; x86_64 assets are the same names but unmeasured.
- A failed background install is not written to the server log — only the job outcome carries it. Noted, not changed.
