# task56: auth-first-message QA monitor + bounded DOM driver

Status: **implemented, UNCOMPILED, native-UNRUN.** No Cargo, no WASM build, no window/server/listener/process, no keyring, no real credentials were used. Only JS was executed (`node qa/native_ipc/panel_driver.mjs --self-test`: 36/36 pass after the reveal_browser addition (section 7); `real_qa_prep.mjs --self-test`: 32/32).

## 1. Published interface (for the `run-real.mjs` owner, agent d502a420-f500-49c)

Endpoint (unchanged): `ws://127.0.0.1:<port>/qa`, page is the client, harness owns the listener. Authoritative code: `qa/native_ipc/monitor_contract.mjs` (`buildCommand`, `StreamValidator`).

### page -> harness `hello` (first message, seq 0)

```json
{"v":1,"type":"hello","run":"<32 lowercase hex, per document>","seq":0,"contract":"aleph-qa-monitor-v1","auth":"<QA root nonce, 16..128 hex>"}
```

`auth` = the validated root nonce from `.qa-owned` (the same nonce the harness wrote). It is an **ownership nonce, not a secret, not proof of IPC, not proof of identity**. Harness must check **exact** `auth === nonce` AND upgrade `Origin === marker origin` (`StreamValidator({expectedAuth, expectedOrigin, requestOrigin})` fails closed: `auth_not_configured`, `origin_not_configured`, `origin_mismatch`, `auth_mismatch`, `run_equals_auth`).
If the monitor cannot install, the only event is `install_failed` (untrusted, no `auth`, no `hello`) and the page ignores all commands.

### harness -> page command envelope (one WS text message per command)

```json
{"v":1,"type":"command","run":"<run from hello>","id":0,"command":"inspect"}
{"v":1,"type":"command","run":"<run>","id":1,"command":"ask_pair"}
{"v":1,"type":"command","run":"<run>","id":2,"command":"submit_ticket","ticket":"<string 1..2048 chars>"}
{"v":1,"type":"command","run":"<run>","id":3,"command":"reload"}
{"v":1,"type":"command","run":"<run>","id":4,"command":"reveal_browser"}
```

Rules enforced page-side: exact key set (5 keys; 6 with `ticket`, and `ticket` ONLY for `submit_ticket`); `run` == the page's run token; `id` integer, strictly `lastId+1` starting at 0 (max 512; a refused command does NOT consume an id); raw message string <= 4096 chars; `command` in the 4 snake_case names; one command in flight (a command arriving while busy is refused); ignored before `hello`, after `install_failed`, after the stream ended. Build with `buildCommand({run,id,action,ticket})` (returns the JSON string; throws fixed codes that never echo the ticket).

### page -> harness replies (fixed enums + booleans only)

```json
{"v":1,"type":"command_ack","run":"...","seq":N,"action":"ask_pair|submit_ticket|reload|inspect|reveal_browser|refused","ok":true|false}
{"v":1,"type":"dom_state","run":"...","seq":N,"tokenWall":b,"tokenInput":b,"tokenSubmitEnabled":b,"tokenRejected":b,"pairingNotice":b,"pairDesktop":b,"pairDesktopEnabled":b,"paired":b,"resource":b,"live":b,"liveStatus":b,"workspace":b,"layoutToggle":b,"layoutToggleEnabled":b,"browserTab":b,"browserTabEnabled":b}
```

- `refused` is always `ok:false` (malformed / unknown / stale id / wrong run / extra key / busy). Absent or disabled targets give `ok:false`, never a silent success.
- `inspect` emits `dom_state` then `command_ack{inspect,true}`.
- `reveal_browser` (fixed, no extra keys, never carries a ticket): clicks the real `[data-testid=layout-toggle]` button ONLY if the workspace pane (`[data-testid=workspace-panel]`, no `workspace-collapsed` class) is not already open (the toggle would otherwise close the Split pane), then polls at most 20 x 25 ms (500 ms) for an ENABLED `[data-testid=workspace-tab-browser]` button and clicks only that. Toggle absent/disabled, or tab absent/disabled after the bound => `command_ack{reveal_browser,false}`. `ok:true` = clicks dispatched, NOT proof the Browser body rendered (use `inspect` / IPC evidence). The `data-testid` attributes were added to `layout_toggle.rs` and `workspace_panel.rs` (attributes only, no behavior change).
- `ok:true` for `ask_pair`/`submit_ticket` means only "the real button was clicked" (clicks dispatched). It is NOT evidence that pairing or token acceptance happened: that needs IPC evidence / a later `inspect`.
- `reload`: ack `true`, then `location.reload()` after 100 ms. The new document gets a **new run token and a new WS connection** (new stream: new hello/auth/ids from 0). `pagehide` emits `monitor_end{page_unload}` on the old stream.
- Harness sequencing: call `validator.expectCommand(action)` right before sending; the validator then requires exactly one matching ack (or `refused`) before the next command, `dom_state` only while an `inspect` is pending, and `finish().complete` is false with a command pending.

### Ticket handling

The ticket travels harness -> page ONLY. Page side: it lives in a handler-local variable (the parsed command is dropped), is written to the real password input through the prototype's native value setter + a bubbling `input` Event, and the local reference is nulled immediately after. It is never emitted, logged, stored in a module variable, or placed in a URL. On a submit timeout the input is reset to `''`. Harness side: keep it in memory only; never put it in argv/env/config/URL/log/artifact.

## 2. Files changed (only the allowed set)

| File | Change |
|---|---|
| `desktop/shell/src/main.rs` | `QA_MONITOR_JS` rewritten: hello+`auth`, fixed command driver (`ask_pair`/`submit_ticket`/`reload`/`inspect`/`reveal_browser`), fixed `SEL` selector table; new `QA_MONITOR_NONCE_PLACEHOLDER`; `qa_monitor_script(port, nonce) -> Option<String>` (re-validates 16..=128 ASCII hex, substitutes the JSON string literal, `None` otherwise); window-builder call site requires BOTH `qa_root::monitor_port()` and `qa_root::monitor_auth_nonce()`, else an eprintln refusal (no monitor). Invoke wrapper unchanged in behavior (same `this`, args, promise object / exception). |
| `desktop/shell/src/qa_root.rs` | `static MONITOR_AUTH: OnceLock<String>`; `fn monitor_auth_for(port, nonce)`; set in `init_from_args` only after ROOT proof and a monitor port; `pub(crate) fn monitor_auth_nonce() -> Option<&'static str>` returns the nonce only if ROOT proof AND the port were set. New unit test `monitor_auth_only_with_port_and_unpublished_before_proof`. |
| `qa/native_ipc/monitor_contract.mjs` | `hello.auth`; new event types `command_ack`, `dom_state`; `ACTIONS`, `ACK_ACTIONS`, `DOM_STATE_KEYS`, limits; `StreamValidator` auth/origin/command-sequencing (see 1); `authEquals` (constant-time on equal length); `buildCommand`; schema doc (`commands`). `validateMonitorChannel` is UNCHANGED and still returns `{ok:true, nonceAuthenticated:false}` (it only sees the connection, never the first message). |
| `qa/native_ipc/panel_driver.mjs` (new) | Import-side-effect-free; `extractMonitorScript`, `renderMonitorScript`, `createMockPage`, `selfTest`; `--self-test` when run directly. |
| `interfaces/webchat/src/components/token_wall.rs` | **Narrow scope addition (allowed):** `data-testid` = `token-wall` (root), `token-wall-input`, `token-wall-submit`, instruction `<p>` = `token-wall-instruction` / `token-wall-instruction-rejected`. Markup attribute only, no behavior change. **Needs WASM rebuild.** |
| `interfaces/webchat/src/platform/wide/views/browser/viewer.rs` | **Narrow scope addition (allowed):** `PairingNotice` gets `data-testid` `pairing-notice` (div) and `pairing-notice-pair-desktop` (button). **Needs WASM rebuild.** |

All four existing files are CRLF; CRLF was preserved. `real_qa_prep.mjs` and `run-real.mjs` were NOT touched.

## 3. What the JS-only self-test proves (`panel_driver.mjs --self-test`, 36 tests)

It extracts the script text from `main.rs`, substitutes placeholders, and runs it in `node:vm` against mocks (WebSocket/document/window/Event/timers). Covered:
- nonce compare: exact only (flipped char, short, long, uppercase, missing, non-string all fail); origin mismatch/unconfigured and `auth`/expectation unconfigured all fail closed; nonce appears ONLY in `hello.auth`, never in a later event;
- unknown/camelCase/eval-ish names, extra/missing keys, wrong run/v/type, bad/replayed/skipped ids, non-JSON, oversize, non-string, bad tickets, busy => `command_ack refused ok:false` and NO DOM action; refused events never echo any command field; a refused command does not consume an id;
- absent / disabled / aria-disabled / non-button => ack `false`; enabled => exactly one click on the fixed selector only; only the fixed `SEL` selectors are ever queried;
- `reveal_browser`: toggle absent/disabled => false without clicks or polling; toggle then tab clicked => true; already-open pane => toggle NOT clicked; collapsed class => toggle clicked; late-enabled tab polled; never-enabled/absent tab => bounded poll (<=20 timers, <=25 ms each, <=500 ms) then false, no click; extra key/ticket/selector or unknown names (`reveal_browser `, `revealBrowser`) => `refused`, nothing clicked; only fixed selectors queried;
- ticket written via the prototype native setter (not the own-accessor trap) + bubbling `input`, then submit click; ticket on no emitted event (success, failure, refusal); submit-disabled timeout => ack `false`, input reset to `''`;
- `install_failed` => no hello/auth, commands ignored, validator marks untrusted and refuses `expectCommand`;
- validator sequencing (`unsolicited_ack`, `ack_action_mismatch`, `inspect_without_state`, `unsolicited_dom_state`, `command_in_flight`, pending => not complete);
- SUPERSEDED (wrapper removed; see `docs/reports/verify-20261007/task56-monitor-seam.md` note; the mock now models immutable internals): historical MOCK-ONLY: invoke wrapper keeps `this`, args, the returned promise object (`===`), sync-thrown error object and rejection; unobserved commands are silent; emitted invoke events carry shapes/booleans only (no url/lease/credential/error text). **This is not evidence about the real Tauri IPC.**

Mutation spot-check (done by hand, not committed): dropping the id-sequence check, the native setter, or `ok:` honesty each makes tests fail. **Not covered by any test:** that the ticket local is nulled after use (a mock cannot observe a closure variable); this rests on code inspection.

## 4. Residual risks / limits (do not over-read the nonce)

1. The nonce is a **QA ownership nonce**, not a secret. The page cannot authenticate the harness: any local process that binds the fixed port first receives `auth` (and later the `ticket`, if the harness sends one to it). Origin check is harness-side only. Accepted for a loopback-only QA channel; mitigated by exact `Origin` + nonce check on the harness side, bounded fixed commands, and the harness sending the ticket only after authenticated `hello` and a matching `Origin`.
2. The nonce also derives the QA keyring service name; treat the `hello` frame as sensitive to the same degree (never log raw hello).
3. `ok:true` on an ack = click dispatched, not outcome. Outcome evidence must come from IPC events (`panel_credential_*`), `dom_state` after the action, or Gateway-side evidence.
4. `PairingNotice` can render twice (decision + launch sections); `ask_pair` clicks the first match only.
5. `dom_state.tokenRejected` is the only failure signal for the token wall; no error text is exposed by design.
6. `reload` starts a new stream (new run token, ids from 0, new hello): the harness needs one `StreamValidator` per connection.
7. A refused ack while a real command is in flight cannot occur if the harness honors `expectCommand`'s one-in-flight rule; if it sent a second command anyway, the late real ack would be `unsolicited_ack` (fail closed, intended).
8. Rust additions (script string, `qa_monitor_script`, `monitor_auth_nonce`, the new test) and the Leptos `data-testid`s have **never been compiled**. The real Leptos DOM/controlled-input behavior with the native setter + `input` event is unverified.

## 5. Needed follow-ups (owners other than this task)

**`qa/native_ipc/real_qa_prep.mjs` (not edited, owner must update; its self-test currently reports 28/32):**
`hello` events in its fixtures have no `auth` and its `StreamValidator` constructions have no `expectedAuth/expectedOrigin/requestOrigin`, so the 4 monitor tests fail (`missing_key` / `auth_not_configured`): fixture `E()` helper ~line 784, hello fixtures ~789, 819, 824, 848, 877-886, `new StreamValidator({credentialNonce: NONCE})` at ~796, 801, 809, 818, 826, 881. Fix: add `auth` to hello fixtures and pass `expectedAuth/expectedOrigin/requestOrigin`. The `monitorNonceAuth: false` prerequisite (line ~42) and the `{ok:true, nonceAuthenticated:false}` assertion (~914) are intentionally left as-is: flip `monitorNonceAuth` only after a real native run authenticates `hello.auth` over the listener. The schema-doc key-name regex test (~918) still passes (verified).

**`run-real.mjs` (d502a420-f500-49c):** listener must (a) take the upgrade `Origin` as `requestOrigin`; (b) construct `StreamValidator({credentialNonce?, expectedAuth: <root nonce>, expectedOrigin: <marker origin>, requestOrigin})`; (c) feed every page message to `push`; (d) before sending each command call `expectCommand(action)` then send `buildCommand({run, id, action[, ticket]})`; (e) send nothing until the first `hello` validated (`finish().authenticated`); (f) never log raw frames containing the ticket or `auth`; (g) one validator per connection (reload => reconnect).

**Native/real verification still required:** rebuild the WASM Panel (testids) and the desktop shell (`cargo check/clippy/test -p aleph-desktop-shell` with `qa-owned-root`, including `qa_root` tests); a real WebView run proving the script installs, `hello.auth` arrives and authenticates, commands drive the real Pair/token-wall DOM, and invoke events appear; then (and only then) decide on flipping `monitorNonceAuth`.

## 6. NOT done

No compile/clippy/Rust tests; no WASM rebuild; no listener or real WebSocket run; no native shell launch; no edits to `real_qa_prep.mjs`, `run-real.mjs`, `Cargo.*`, the dist bundle; no commit/reset; `monitorNonceAuth` NOT flipped; no verification of real Tauri IPC `this`/promise behavior (only a mock); no cross-browser/WebView2 verification of the native-setter approach; the ticket-nulling is not test-covered.

## 7. reveal_browser seam (QA-only) - native NOT run

Changed: `desktop/shell/src/main.rs` (QA_MONITOR_JS), `qa/native_ipc/monitor_contract.mjs` (ACTIONS, DOM_STATE_KEYS, MONITOR_SCHEMA_DOC), `qa/native_ipc/panel_driver.mjs` (self-test only), `qa/native_ipc/real_run_core.mjs` (`evaluateSeam` source check also accepts the single `=> "<id>",` arm of `body_testid`), `interfaces/webchat/src/components/layout_toggle.rs`, `interfaces/webchat/src/components/workspace_panel.rs` (data-testid attributes only). `monitorNonceAuth` stays false (real_qa_prep fail-closed). Verified only by node self-tests (`panel_driver.mjs --self-test`: 36/36; `real_qa_prep.mjs --self-test`: 32/32); no Cargo, no WASM, no Tauri window, no real DOM run.
