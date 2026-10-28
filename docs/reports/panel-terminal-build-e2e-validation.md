# Terminal Build E2E Validation / Terminal 真实构建 E2E 验证

Date: 2026-10-28
Branch: `terminal-build-e2e`
Worktree: `D:/Workspace/Aleph/.worktrees/terminal-build-e2e`

The supplied integration-point text named `f48991a5b`; the clean shared `main` and this worktree actually resolve to `3ac50c1be5c3a613195cb76c080ef7d93f99d9ba` (`docs(gateway): plan compatibility verification closure`). This report records the revision that was actually built and exercised. The earlier `docs/reports/panel-terminal-live-parity-closure.md` remains historically intact.

## Verdicts / 结论

```text
component: PASS
production_host_loopback: PASS
live_gateway: BLOCKED
```

`component` and `production_host_loopback` are independent evidence layers. The loopback result is not promoted to live Gateway parity.

## Build provenance / 构建来源

The candidate was generated from the new worktree without copying the shared checkout's ignored `.panel-tabs` output.

- Build cwd: `D:/Workspace/Aleph/.worktrees/terminal-build-e2e/interfaces/webchat`
- Build command: `bun scripts/build-panel-tabs.mjs --clean "--output=../../.superpowers/sdd/2026-10-28-terminal-build-e2e-implementation/candidate"`
- Output root: `D:/Workspace/Aleph/.worktrees/terminal-build-e2e/.superpowers/sdd/2026-10-28-terminal-build-e2e-implementation/candidate`
- Revision: `3ac50c1be5c3a613195cb76c080ef7d93f99d9ba`
- Profile: `wasm-release`
- Build id: `612edbe3001f4339`
- Manifest entry: `./tabs/terminal/612edbe3001f4339/terminal.js`
- Host descriptor: `aleph / terminal`, API major `1`, trust `verified`, status `enabled`
- Source input hash ledger: `candidate/build.json` and `candidate/build-inputs.json` (`source_input_hashes`)
- Tools: Bun `1.4.4`, Cargo `1.99.1`, rustc `1.99.1`, wasm-bindgen `0.2.129`, wasm-opt `133`, Tailwind `4.3.3`

The staged deployed asset list was:

```text
index.html
aleph_panel.js
aleph_panel_bg.wasm
panel_loader.js
tailwind.css
snippets/aleph-panel-4f6e070d4bce8856/inline0.js
tabs/terminal/612edbe3001f4339/terminal.js
tabs/terminal/612edbe3001f4339/terminal_bg.wasm
tabs/terminal/612edbe3001f4339/snippets/panel-tab-terminal-c09656c1df86b3e8/inline0.js
```

The temporary candidate target and dependency ledgers were excluded from deployed-asset assertions and first-screen claims. `build.json.revision`, manifest entry, descriptor identity, build id, source hashes and final asset paths agree. Artifact checks reject stale build ids, missing Terminal WASM, wrong descriptor identity, wrong public base, and entry traversal outside the staged root. The final HTML contains no unprocessed Trunk asset marker.

Every cargo subprocess was preceded by `python qa/browser_live/check_memory.py` from the worktree root and ran from its owning package directory. The build log records `ALLOW` for the host and Terminal builds.

## Component evidence / 组件证据

The package test scripts now require an explicit `PANEL_OUTPUT_ROOT`; the commands below pass the isolated candidate directly so they cannot silently consume historical `.panel-tabs` output.

From `interfaces/webchat`:

- `PANEL_OUTPUT_ROOT=../../.superpowers/sdd/2026-10-28-terminal-build-e2e-implementation/candidate node --test tests/panel-build-artifacts.test.mjs tests/panel-build.test.mjs` → **11 passed / 0 failed**.
- `PANEL_OUTPUT_ROOT=../../.superpowers/sdd/2026-10-28-terminal-build-e2e-implementation/candidate node --test tests/panel-terminal-protocol.test.mjs` → **1 passed / 0 failed**.

The protocol fixture independently verified Chat startup does not request Terminal assets, versioned module reuse, exact `pty.screen`/`pty.exit` topics, sequence-gap attach recovery, UTF-8 Ctrl-C and bracketed-paste boundaries, selected-session exit fallback without closing another PTY, empty-list spawn/attach, native Canvas `fillText`, wrong-topic filtering, post-baseline resize, epoch replacement, and `pty.list` failure without spawn.

Additional loader and gate checks from `interfaces/webchat`:

- `node --test tests/panel_loader.test.mjs tests/panel-build-memory-gate.test.mjs` → **9 passed / 0 failed / 1 skipped**. The skipped case explicitly requires generated feature glue; the memory-gate rejection check passed.

The wrong-topic assertion was made effect-level: it records the delivered topic and asserts that the wrong-topic marker never reaches the renderer. This avoids treating unrelated cursor/animation repaint activity as a protocol failure.

## Production host loopback / 生产 Host 回环证据

The Playwright runner consumed only `PANEL_OUTPUT_ROOT` and served the candidate's final `index.html`, host JS/WASM, loader, CSS and versioned Terminal JS/WASM. It used the real production host, `PanelShell`, `PanelLoader`, `PanelSdk` and Terminal module; only the HTTP/WebSocket Gateway boundary was fixture-backed.

Observed browser asset paths included:

```text
/aleph_panel.js
/aleph_panel_bg.wasm
/panel_loader.js
/tabs/terminal/612edbe3001f4339/terminal.js
/tabs/terminal/612edbe3001f4339/terminal_bg.wasm
```

The fixture passed the complete production browser chain for `/` and direct `/terminal`, while the same server returned positive staged HTML for `/`, `/panel/`, `/terminal`, and `/panel/terminal`. The component fixture separately covers both Chat and Settings before Terminal navigation. The production host initialized the current host WASM and loaded the current build's versioned Terminal module. `/panel/` and `/panel/terminal` are retained as positive HTTP route checks; the current Panel router selects the modular Terminal route at `/terminal`, so they are not reported as additional modular mount runs.

Loopback WebSocket evidence included `connect`, exact event subscriptions, `pty.list`, `pty.attach`, screen delivery with visible `Gateway event ✓`, UTF-8 Ctrl-C payload `Aw==`, a new `pty.resize` geometry after the baseline report, selected-session exit and fallback attach, and replacement-socket resubscription followed by visible `Reconnect marker ✓`. The fixture asserts that fallback and route teardown do not call `pty.close` for a non-test-owned PTY. The final run reported no page errors, no failed requests, and no HTTP response status >= 400; cleanup closed the page, browser, socket and fixture server.

Command from `interfaces/webchat`:

```text
PANEL_OUTPUT_ROOT=../../.superpowers/sdd/2026-10-28-terminal-build-e2e-implementation/candidate node --test tests/panel-terminal-host.test.mjs
```

Result: **1 passed / 0 failed**.

## Rust and repository checks / Rust 与仓库检查

From the worktree root, each command below had a preceding memory gate with exit 0; Cargo ran from `interfaces/webchat`:

- `cargo check --locked -p aleph-panel --lib --no-default-features --features panel-modular` → PASS.
- `cargo test --locked -q -p aleph-panel --lib --no-default-features --features panel-modular` → **1578 passed / 0 failed**.
- `cargo clippy --locked -q -p aleph-panel --lib --no-default-features --features panel-modular -- -D warnings` → PASS.
- `git diff --check` → PASS.
- Root `bun run check` → unavailable; the root `package.json` has no `check` script. The affected package checks above were run instead.

The submission hook was inspected: it can invoke Cargo independently. Its result is not used as a substitute for the explicit memory-gated package checks above.

## Live Gateway gate / 真实 Gateway 闸门

```text
live_gateway: BLOCKED
reason: no authorized disposable Gateway/project/session/credential/cleanup scope
```

No live Gateway endpoint, credential, project, session, PTY, health/auth request, WebSocket, or process-control operation was attempted. No live resource was created, so no live cleanup claim is made. The loopback fixture is not live evidence.

## Scope and deferred work / 范围与后置工作

Completed here: build-aware artifact validation, clean source build, production-host loopback E2E, component regression evidence, memory-gated Rust checks, and non-secret build/browser/RPC ledgering.

Explicitly deferred: a second Tab, Gateway Bound Stage C, default release flip, community lifecycle, `src/harness/`, Capability/ZahirFacade runtime, PTY server business logic, other Tab business logic, full mobile/accessibility/release hardening, full rollback matrix, and live Gateway acceptance.
