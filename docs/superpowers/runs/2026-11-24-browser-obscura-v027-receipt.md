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

## Capability table, final / 能力表终值 (obscura, measured 2026-11-25, v0.2.7)

| Row | Value | Reading |
|---|---|---|
| js_dialogs | Unsupported | no `javascriptDialogOpening` for alert/confirm/prompt (only `beforeunload`); `handleJavaScriptDialog` -32601 |
| drag | **Supported** (was Unsupported) | six-event mouse gesture → `dropped:mouseup`; **no HTML5 `dragstart`/`drop`** — a drop-only target does not react |
| file_upload | Unsupported | still gated by `--allow-file-access`, which Aleph never passes |
| pdf / insert_text / ref_precheck | Supported | unchanged |
| effect_probe | **Supported** (was Unsupported) | readback after a self-navigating click still clean null, but the nonce is gone on the new doc → Skipped, not a false miss |
| network_interception | Unsupported | pause now holds and fulfill answers ok, but the page renders an empty body |
| screencast | Supported | usable frames |
| error_events | Unsupported | async throw → no `Runtime.exceptionThrown` within 10 s |

caps stage: table == measurement on all 10 rows, `measured_on` == binary (`v0.2.7`).

## Other readings / 其他测量

- **Screenshot geometry:** `#geo` = [137, 211, 53, 29] in box model, client rect and screenshot pixels (image 1280 = CSS 1280) on both engines → no screenshot note added. One absolutely positioned box only; normal-flow layout not measured.
- **loaderId (⑯b):** the stale loader did not reproduce; the main-frame event carries the new loader. Read order `result.loader_id.or(outcome.loader_id)` kept.
- **Two sessions per page:** `<target>-session` (createTarget) and `<target>-session-<n>` (attachToTarget); page-started navigation lands on either. Before the fix, `click` failed 7 of 11 runs; after it, 6 of 6 on v0.2.7, 0 fails on v0.2.2.
- **Visibility:** v0.2.7 resolves inherited `visibility` (upstream f019ebe), so Aleph's own cascade of it was removed. An operator-pinned v0.2.2 loses that cascade (documented in `fetch_obscura.rs`).
- **`DOM.disable`:** v0.2.7 accepts it; v0.2.2 answers -32601. Box-model / resolveNode still work after it.

## Verification / 验证

- `cargo clippy -p alephcore -p aleph-cdp --all-targets -- -D warnings`: clean.
- `cargo test -p aleph-cdp`: 97 passed.
- `cargo test -p alephcore --lib -- browser runtimes builtin_tools::browser_tools`: 1355 passed, 4 failed. All four also fail on clean `main` 99090adce (`browser::live::issuer` ×2, `browser::live::owner_registry` ×1, `browser_tools::tabs::no_browser_tool_formats_a_raw_backend_error`) and are not from this branch.
- `qa/browser_dual/run.sh` on v0.2.7, after b636e01b0:

| Stage | PASS | FAIL | Note |
|---|---|---|---|
| provision | 21 | 0 | run at 21d362a2f; later commits touch comments only. Needs a GitHub API budget (unauthenticated, hourly) |
| open | 19 | 0 | |
| snapshot | 12 | 0 | |
| click | 26 | 0 | |
| stall | 17 | 0 | KNOWN GAP still present on 0.2.7: the snapshot path calls a busy engine "stopped" |
| reap | 13 | 0 | |
| caps | 34 | 0 | |
| switch | 29 | 0 | |
| escape | 14 | 0 | |

## Independent review / 独立审查

A read-only branch review (openai-codex/gpt-6.1-sol, 2026-11-25) blocked the merge on one P1 and raised one P2. Both are fixed.

- **P1 — iframe read-back in the wrong realm.** The install runs `Runtime.callFunctionOn` on the target node, so its `window` is the node's realm. For a target in a same-renderer iframe or `srcdoc` frame, which `resolve_target` admits, that realm is the frame's own global. The read-back was a page-level `Runtime.evaluate`, so it read the main frame and found no nonce. Every iframe action was therefore reported `Skipped`, real misses included, and the frame-side listeners were never removed. Fix: `ArmedProbe { nonce, object_id }` in `src/browser/cdp_backend/actions.rs`; the read-back is now a `Runtime.callFunctionOn` on that same node. After a navigation the read-back through the old node was measured, not assumed: Chrome 158 main-frame link 3/3 and obscura v0.2.7 caps 3/3 → call error ("Cannot find context with specified id" on Chrome) → `Unknown` → `Skipped`; Chrome 158 `srcdoc` iframe link 3/3 → `click` from the old realm → `Verified` (correct). The one unsound answer — a clean `null` in a realm that kept the nonce, i.e. a false `EffectNotDelivered` — was not seen on either engine and is not guaranteed against; caps premise 3 now reads back through the installed node exactly as production does and refuses obscura's row on it (re-review P2, 2026-11-25). Test: `the_readback_runs_in_the_realm_the_probe_was_installed_in`. Real Chrome 158 check, a `srcdoc` iframe button probed by raw CDP: the old main-frame read gave `{otherDocument: true}` on both a hit and a miss; the read on the node gave `click` and `null`.
- **P2 — the geometry reading could not fail caps.** `qa/browser_dual/caps.py` now `led.check`s `geometry_agrees`, which pins the "no screenshot coordinate warning" conclusion.
- After the fixes: `cargo test -p alephcore --lib -- browser::cdp_backend` 186 passed; clippy clean; QA on v0.2.7: click 26, caps 35, switch 29, escape 14, open 19, snapshot 12, all with 0 FAIL. After the re-review's navigation change to premise 3: caps 36 PASS / 0 FAIL, three runs.

## Deliberately not done / 刻意不做

- No automatic engine switching; the model still decides (R7/R10).
- No upstream obscura patches; no `--allow-file-access`.
- `fetch_obscura.rs` stays: v0.2.7 `DOMSnapshot` still fabricates geometry (`y = i*18`).
- `fetch-probe.mjs` / `qa-events-probe.mjs` were not re-run; network_interception and error_events rest on the caps line.
- No Linux / Windows run; x86_64 assets are the same names but unmeasured.
- A failed background install is not written to the server log — only the job outcome carries it. Noted, not changed.
