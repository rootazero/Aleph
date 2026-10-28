# Terminal Build E2E Design / Terminal 构建端到端设计

## Intent / 意图

Prove that a clean modular Panel candidate built from the current source revision is loaded by the final staged HTML through the real production host WASM, production navigation, PanelShell, PanelSdk, PanelLoader, and versioned Terminal JS/WASM. Keep the existing component evidence separate, and do not upgrade loopback evidence into real-Gateway parity.

证明当前源码 clean build 的 Panel modular candidate 确实由最终 staged HTML 经真实 production host WASM、生产导航、PanelShell、PanelSdk、PanelLoader 和 versioned Terminal JS/WASM 加载。组件证据保持独立，loopback 证据不得升级为真实 Gateway parity。

## Evidence boundary / 证据边界

- `component`: the existing protocol fixture using the newly built versioned Terminal module; its verdict is reported independently.
- `production_host_loopback`: the final staged `index.html` and real production host stack, with Gateway behavior simulated only at the loopback HTTP/WebSocket boundary.
- `live_gateway`: remains `BLOCKED` unless the operator separately supplies an explicitly authorized disposable Gateway/project/session/credential and cleanup scope. No such target is authorized in this design.

## Baseline / 基线

The supplied integration point was `f48991a5b`, but both the shared `main` and the new worktree resolve to `3ac50c1be`. The build ledger must record the actual tested HEAD `3ac50c1be`; it must not claim the supplied revision was tested. Work occurs only in `.worktrees/terminal-build-e2e` on branch `terminal-build-e2e`.

## Goals / 目标

1. Build once from the current source into a new isolated output root, with an isolated temporary Cargo target and no copied historical `.panel-tabs` assets.
2. Record a non-secret ledger containing revision, profile, build ID, source input hashes, tools, manifest/descriptor identity, asset inventory, and every Cargo memory-gate result.
3. Parameterize the existing component and production-host fixtures with an explicit output root so they consume exactly that build.
4. Assert artifact consistency and real browser requests, production navigation, RPC/event behavior, route coverage, epoch fencing, failure isolation, and test-owned cleanup.
5. Report the three verdicts independently and update only facts actually verified.

## Non-goals and anti-goals / 非目标与反目标

- No second Tab migration, Gateway Bound Stage C, default release switch, community lifecycle, harness, Capability/ZahirFacade runtime, PTY server business logic, or unrelated Tab changes.
- No synthetic replacement HTML, Shell, SDK, DashboardState, injected navigation, or direct module API calls may be used as production-host evidence.
- No real Gateway access or resource cleanup without the mandatory live authorization.
- No preventive production refactor. Production code changes require a repeatable product defect, a minimal RED, a local fix, a rebuild, and regression evidence.
- A build PASS or loopback PASS must never be presented as live Gateway PASS.

## Approaches / 方案

### Chosen: parameterize the existing fixtures

Use an explicit output-root argument/environment value in the existing protocol and host fixtures, and add a build-aware artifact/host runner where useful. This reuses the existing Gateway boundary fixture and assertions, keeps component and production-host verdicts separate, and minimizes parallel infrastructure.

Trade-off: the existing tests must distinguish component-only behavior from production-host behavior and avoid accumulating unrelated responsibilities. The artifact consistency checks belong at the build/host boundary rather than inside the component verdict.

### Rejected: add a separate full production-host runner

This would give the cleanest file-level separation but duplicate the existing loopback server, browser lifecycle, request/RPC capture, and much of the host test. The extra maintenance cost is not justified when `createPanelGatewayFixture(staged)` already accepts an arbitrary staged root.

### Rejected: keep fixed `.panel-tabs` consumption

Rebuilding into the conventional ignored directory would be shorter, but it weakens the clean provenance boundary and makes accidental stale-asset consumption harder to detect. It does not satisfy the explicit isolated-output requirement as directly.

## Implementation shape / 实施形状

1. **Artifact and ledger validation**
   - Invoke the existing build script with `--output=<isolated-root>` from `interfaces/webchat`.
   - Resolve the actual HEAD independently and assert `build.json.revision`, `manifest.json.build_id`, and entry build ID agree.
   - Assert the manifest entry and Terminal descriptor identity agree with `aleph/terminal`, the versioned JS/WASM files exist, host glue/WASM, loader and CSS exist, and final HTML has no unprocessed Trunk assets.
   - Assert the deployed inventory excludes the temporary target and first-screen host HTML does not reference the Terminal module.
   - Add negative fixture checks for stale build ID, missing asset, wrong descriptor, and wrong public base. These are fixture validations, not live or production-host PASS evidence.

2. **Component regression**
   - Let the existing protocol fixture accept the same explicit output root, retaining its synthetic page/SDK role and independent verdict.
   - Preserve existing RPC, UTF-8, resize, exit/fallback, reconnect/epoch, late-result, failure isolation, and teardown coverage; add only demonstrated effect-level gaps.

3. **Production-host loopback**
   - Let the existing host fixture accept the same explicit output root and start Playwright from its final staged `index.html` through the loopback HTTP server.
   - Record non-secret asset paths, page errors, failed requests, RPC methods/events, and cleanup state on both success and failure.
   - Assert Chat/Settings do not prefetch Terminal, real navigation mounts the production Shell/SDK/Loader, requested Terminal assets carry the current build ID, and the exact manifest entry is requested.
   - Cover `/`, `/panel/`, direct `/terminal`, and `/panel/terminal` according to the real router contract. If `/panel/terminal` exposes a repeatable routing defect, establish RED before any local production fix.
   - Exercise Gateway-shaped connect/subscription/list/attach/spawn as needed, visible screen output, UTF-8 input, post-baseline resize, selected exit/fallback, and reconnect/epoch fencing.

4. **Verification and cleanup**
   - Run `python qa/browser_live/check_memory.py` from the worktree root before every Cargo subprocess, including every build-script invocation; accept only exit 0.
   - Run Cargo serially from its owning package directory. Audit commit hooks separately and never describe hook-spawned Cargo as compliant package verification.
   - Close browser contexts, WebSocket connections, servers, timers/observers, and only test-owned resources. Route teardown must not close an external PTY.
   - Run `git diff --check` and applicable independent read-only review. No merge, push, or release flip.

## Reporting / 报告

Update the bilingual validation report with exact commands and working directories, memory gates, actual revision/build ID, asset requests, initialization/RPC/event evidence, cleanup evidence, and remaining scope. Update `TERMINAL_RUNTIME` and `FEATURE_LOCATOR` only with verified facts. The live verdict remains:

```text
live_gateway: BLOCKED
reason: no authorized disposable Gateway/project/session/credential/cleanup scope
```

## Builder discretion / 留给实现者

The exact environment variable/CLI spelling, helper placement within the existing test files, and result JSON formatting are implementation details. They must not weaken the output-root provenance, three-verdict separation, secret hygiene, Cargo gate, or cleanup requirements.
