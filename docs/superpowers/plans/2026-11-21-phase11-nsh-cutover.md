# Phase11: NSH desktop cutover and Tauri retirement
# Phase11：NSH 桌面切换与 Tauri 退役

**Status / 状态:** Ready for implementation planning review; implementation begins in a new session on local `main`. This document does not switch the installed desktop or remove Tauri. / 下一会话从本地 `main` 开始，本文未执行切换或删除。

**Goal / 目标:** Ship NSH as the desktop host on macOS, Windows and Linux, preserve the canonical Leptos/WASM Panel and existing desktop contracts, then remove the Tauri shell and its active dependencies after at least two weeks of evidenced dual operation. / 三平台替代、默认切换、至少两周真实双轨验证，最后全删 Tauri；共享 Panel 保留。

**Architecture / 架构:** One Rust Core, one Panel business UI, one host contract with platform implementations. NSH owns windows and native I/O; Core owns reasoning and service authority; Panel owns navigation, Chat and settings UI. Reuse the current protocol, capability, credential and service boundaries. Do not introduce a second Gateway client, business projection, permission system or registry in the shell. / 延续 R2 choice A，保持一核、一份业务 UI 与清晰的宿主生命周期。

**Trade-offs / 取舍:** Keeping the Panel preserves behavior but retains the WASM build and requires migrating its native IPC consumers. Three-platform replacement and a real 14-day overlap take longer than a macOS-only switch. Retaining both hosts during the overlap increases build/QA cost; deleting Tauri afterward removes the live fallback, so signed recovery artifacts and documented restoration must exist first. / 不以删代码速度牺牲现有平台、凭据、升级与回退契约。

**Execution / 执行:** Implement directly unless delegation is authorized in the active session. Use sequential SPEC then distinct QUALITY review for the frozen migration contract and final implementation. Review does not replace runnable checks or native evidence. / 不把可自动核验的工作推给用户。

## 1. Authority and explicitly approved decisions / 依据与已确认决策

Read the [restart prompt](../prompts/2026-11-21-phase11-nsh-cutover-restart.md), [NSH master](../specs/2026-11-05-nsh-desktop-refactor-design.md), [Phase9 R2 decision](../runs/2026-11-16-phase9-nsh-r2-boundary.md), [official Phase10 boundary](../specs/2026-11-20-phase10-official-nsh.md), [Phase10 closeout](../runs/2026-11-21-phase10-closeout.md) and [approved amendment](../runs/2026-11-21-phase10-acceptance-amendment.md).

The operator requested Phase11 to switch the desktop to NSH and delete Tauri, and explicitly selected both of these gates on 2026-11-21:

1. Preserve the master's **at least two weeks of dual operation** before removal. An automated stress run, old Phase10 history, a future-dated receipt or idle elapsed time is not two weeks of use. / 保留至少两周双轨，不用加速计时或历史样本替代。
2. Complete replacement on **macOS, Windows and Linux before deleting all Tauri**. Do not silently stop shipping a platform or describe a partial switch as global removal. / 三平台完成后全删，不擅自缩减产品支持面。

The master is a historical v0.4 design with its own Phases 0–6. Phase11 here is a new bounded host-migration phase, not a claim that the master's native UI Phases 3/4 or Plugin Runtime Phase5 are complete. The master's Phase6 instruction to remove `interfaces/webchat` is superseded by the later R2 choice A: retain the canonical shared Panel. The migration-specific gates in this plan replace the old native-UI prerequisite for retiring this host; they do not mark native-UI work complete. / 本期接替旧壳职责，不冒充原生业务 UI 已迁移。

Keep `desktop/macos`, `desktop/windows`, `desktop/linux`, `desktop/shared`, shared client/protocol and Swift/native helpers unless a separately proved dependency is exclusively Tauri-owned. Retiring Tauri does not retire camera, voice, screenshot, native IPC or other platform capabilities. / 禁止按目录名批量删除仍在使用的原生能力。

## 2. Actual starting point / 实际起点

Phase10 is **complete only within its approved local macOS slice under the amended contract**. See the [closeout integrity receipt](../runs/2026-11-21-phase10-closeout-integrity.json). The installed official NSH `0.6.0 (5541637)` and crates.io `=0.6.0` dependencies are the starting baseline; the old 0.5 fork is historical.

- `desktop/nsh-shell/src/main.rs` currently implements macOS; its other-platform entry is not a production replacement. Official Windows/Linux feasibility and packaging are **not proved**. / 不把占位入口算作跨平台支持。
- `desktop/nsh-shell/src/shell_host.rs::HostSettings::from_inputs` defaults to the local Panel and only accepts the paired alternate endpoint/nonce in debug QA. It is not the shipped Tauri full/lite/remote connection system. / 生产连接设置、首次启动与服务管理仍需补齐。
- Current NSH resident close/reopen/Quit, tray, hotkey, exact deep links, real notification card action and ordinary placement have macOS evidence. Physical multi-display topology remains **user-deferred, NOT TESTED** in Phase10. Reassess that gap for Phase11's broader migration contract; do not silently inherit a PASS or erase the deferral. / 新阶段需要明确处置该未测项。
- Original retained G7 matrix `.phase10-evidence/g7-official-matrix.Ji0duOcq/matrix.json` stays `VALID / NOT_ACCEPTED`. Separate [amendment JSON](../runs/2026-11-21-phase10-acceptance-amendment.json) accepts the same full observations with only Quit median/p95 limits changed to `1.30×`; this is not an optimization. Other latency limits remain `1.10× / 1.20×`, RSS median `1.10×`, thread median delta `2`. / 原失败回执不改，不全局放宽。
- Phase10 did not deliver production updater, full bundled-daemon supervision, remote credential migration, autostart parity, three-platform release acceptance or Tauri deletion. Notification OS denial is not a zero-resource feature-disable switch. / 这些是本期需要审计和实现的真实缺口。

Historical measurement HEAD `0e4e1d4ebaa6e6dee8d6af9928f969d2e90f42b7` remains inside receipts. The restart prompt pins the new committed handoff on `main`; do not replace receipt identities with that newer commit. / 实测身份与交付提交身份分开。

## 3. Global constraints / 全局约束

- Work from the actual local `main` git root. No reset, pull, rebase onto `origin/dev`, deletion of worktrees, or cleanup of historical evidence. Preserve unrelated edits. Inspect status before staging and use exact paths; never `git add .` over local evidence/build roots.
- Do not read or modify `.claude/settings.json`, root `config.lock` or root `config.toml`. Do not log tokens, credential payloads or raw rejected URLs. / 保护既有私密文件。
- Before Cargo: `python3 qa/browser_live/check_memory.py --required-bytes 4294967296` must report `ALLOW`; run builds/checks/tests serially with `--offline --locked --jobs 1` where supported. A missing cache is a visible blocker, not permission to silently change the resolver. Do not run Cargo/assets/native QA concurrently with timed comparison.
- Use official release APIs. No historical fork fallback, toolkit source patches, VM workarounds or unsupported manipulation of NSH-owned windows. If a different official version is needed for platform support, present exact API/dependency evidence and freeze that change in the migration spec before implementation. / 版本变更不得偷换 Phase10 证据。
- Any externally composable new operation follows existing Capability/tool contracts. Reuse existing typed protocols and service APIs; do not add a general RPC tunnel or deterministic replacement for model reasoning. Follow [redlines](../../reference/REDLINES.md), [security](../../reference/SECURITY.md) and [tool system](../../reference/TOOL_SYSTEM.md).
- No native business UI rewrite, new daemon ownership model, plugin runtime or credential-store redesign merely to remove Tauri. Preserve semantics; remove only framework coupling. Document concrete unavoidable differences before changing behavior.
- Next-session authority covers local implementation, builds, QA and the planned default cutover once its gates pass; use the operator's existing authorization rather than asking again about the agreed cutover. Publishing, remote push, new signing trust, secret rotation and destructive credential migration require their own applicable authorization. Prepare concrete artifacts first. / 按已批准目标执行有闸门的默认切换；不将其扩成未经批准的发布或数据销毁。

## 4. Acceptance gates / 验收闸门

| Gate | Required result / 必须达到 | Evidence / 证据 |
| --- | --- | --- |
| P11-A Inventory/spec | Every active Tauri command, frontend consumer, plugin, helper and release product has a retained owner or explicit disposition | Source-backed inventory, signed-off migration contract, each callsite linked to its new handler/rendered UI; no unsupported silent omission |
| P11-B Three platforms | Official NSH replacement launches and works on macOS/Windows/Linux; full/lite and supported local/remote modes remain available | Native per-platform receipts and build-only artifacts, actual OS/version/architecture; cross-compilation alone is insufficient |
| P11-C Contracts/security | Credentials/target changes, native permissions, filesystem/dialogs, external links, autostart, updater, service controls and platform helpers preserve their contracts | Boundary/negative tests and real delivery checks; no fake `__TAURI_INTERNALS__`, token-in-URL workaround or no-op success |
| P11-D Runtime | Shared Panel stream/reconnect/history/Artifacts and shell lifecycle work at normal/narrow sizes; shell Quit leaves external service intact | Fresh comparable blocks and native terminal/observed-member settlement; strict source/build/asset provenance |
| P11-E Default/recovery | NSH is selected by desktop launch/build routes on all three platforms; old Tauri remains available only as a deliberate rollback during overlap | Full/lite install/build inspection, actual default launch, URL association checks and recovery drill; no second singleton owner |
| P11-F Dual run | At least 14 calendar days of actual dual availability/use after all three platforms qualify; no unresolved required regression | Dated per-platform operation journal, start/end identities, actual activities/issues/fixes/rechecks; durable artifacts cannot fabricate elapsed time |
| P11-G Removal | No active Tauri package, desktop crate, IPC dependency, build/CI/release invocation or frontend fallback remains | Workspace/lock/dependency audit, native builds and regression checks after removal, exact deletion diff; historical receipts remain |
| P11-H Handoff | SPEC then distinct QUALITY accepted, gate matrix and all remaining limitations current | Reproducible acceptance/identity/link checks and separate master-phase status; no reviewer assertion substitutes for test evidence |

Fresh runtime comparisons retain the proven sampling/order contract: Tauri40 then NSH40, fresh Gateway/provider/session per round, 10 warmups + 30 measured, actual 100ms owned-resource samples and p95 rank29. On macOS use the approved Quit-only limits above. Freeze baselines/limits for Windows/Linux from real supported hosts before timing; no unapproved numeric relaxation or reused macOS measurement. Retain a runnable baseline artifact before deleting comparator source. A valid rejected block is rerun only after a justified candidate change; invalid collector blocks restart from zero, with no sample splicing. / 跨平台新结果不能由本机 Phase10 结果外推。

## 5. Ordered implementation tasks / 按序实施

### Task 1 — Establish handoff and freeze the migration contract / 起点与契约

**Read:** This plan, restart prompt, full NSH master, Phase10 receipts, [desktop shell](../../reference/DESKTOP_SHELL.md), [Swift bridge](../../reference/DESKTOP_BRIDGE.md), [product topology](../../reference/PRODUCT_TOPOLOGY.md), [platform routing](../../reference/SUBSYSTEM_ROUTING.md), [Windows](../../reference/WINDOWS_RUNTIME.md), [Linux](../../reference/LINUX_DESKTOP.md), [release](../../reference/RELEASE.md), [development](../../reference/DEVELOPMENT.md), security and tool references.

**Inspect:** `Cargo.toml`, `Cargo.lock`, `desktop/nsh-shell/`, all `desktop/shell/src/*.rs`, `desktop/shell/tauri*.json`, `desktop/shell/capabilities/`, `.github/workflows/{aleph-app-release,aleph-core-ci,rust-doctor}.yml`, `justfile`, `build.rs`, `scripts/{stage_shell_placeholders.sh,check_webview_baseline.mjs}`, `interfaces/webchat/webview-baseline.json`.

- [ ] Verify `main`, committed Phase10 handoff, protected paths, unrelated edits and available machines/toolchains. Record current identities; do not reset to measurement HEAD.
- [ ] Query the existing Graphify index before new source exploration; rebuild only if genuinely needed. Census active source/commands/CI, not just `Cargo.lock`. The planning census found 126 tracked Tauri-reference files outside docs/dist; 17 are Panel consumers. These are discovery hints, not a frozen expected count.
- [ ] Create `docs/superpowers/specs/2026-11-21-phase11-nsh-cutover.md` and a versioned inventory under `docs/superpowers/runs/`. Each row names old caller, authority, new boundary, actual handler/UI, failure behavior, lifecycle and verification. A new file path is proposed until created.
- [ ] Prove official NSH platform support and public API feasibility with small isolated experiments. If any required platform is unsupported, retain Tauri and record a concrete blocker/official-version proposal; do not pretend the existing no-op entry works.
- [ ] Freeze full/lite/remote products, credentials/storage, updater trust, autostart, single-instance/URL and recovery behavior; obtain ordered contract reviews before implementing new boundaries. Do not introduce speculative abstractions to fill the table.

### Task 2 — Migrate Panel native calls through one typed host boundary / 统一原生调用边界

**Files:** `shared/protocol/src/panel_shell.rs`, `interfaces/webchat/src/{app.rs,app/shell_bridge.rs,platform_host.rs,context.rs}`, `interfaces/webchat/src/api/{native_credentials,fs}.rs`, `desktop/nsh-shell/src/{main,panel_bridge,shell_host}.rs` and `panel_bridge.js`. Read existing `desktop/shell/src/{keyring_bridge,connection,connect_setup,external_link,webview_perms,perm_monitor}.rs` first.

**Interface:** Existing `BridgeIpc` and typed Panel messages → authenticated/document-bound NSH host → canonical native/service owner. Preserve single-source credential leases, attempts, endpoint matching, invalidation and bounded admission; no broad arbitrary `invoke` proxy.

- [ ] Migrate each `TauriIpc` consumer, shell marker and direct `__TAURI_INTERNALS__` invocation to the typed host contract. Browser behavior remains explicit; no fake Tauri global to avoid doing the migration.
- [ ] Cover settings autostart, connection setup, directory/project selection, notifications, command palette, layout/mode/team controls and voice/audio consumers identified by the actual census. UI remains in the Panel. Required controls cannot silently vanish because the marker changed.
- [ ] Keep origin/subframe checks, secret closure, document generation, request matching, bounded input and outcome-unknown handling from Phase10. Verify rejected requests have no effect and cross-target stale credentials cannot leak.
- [ ] Verify native capability absence/denial/error and browser fallback separately; do not treat a native credential failure as permission to silently downgrade authentication.

### Task 3 — Production connection and daemon ownership / 生产连接与服务边界

**Files:** `desktop/nsh-shell/src/{main,shell_host}.rs`; narrowly named new modules only when needed. Reuse logic from `desktop/shell/src/{daemon,connection,connect_setup,gateway_probe}.rs` and the supervisor in `desktop/shell/src/main.rs`; read `src/bin/aleph-server/commands/{service,start}/` and shared client contracts before moving ownership.

**Interface:** Existing full/lite/local/remote target configuration → canonical service controls and readiness → Panel navigation. OS host is I/O; Core remains authoritative.

- [ ] Supply production launch arguments/configuration independent of debug QA nonce input. Preserve first launch, unavailable/foreign port errors, remote TLS and credential binding, target change/reconnect and existing persisted-state compatibility.
- [ ] Full app can locate/stage its daemon and platform helper and use existing supported service operations. Lite app does not acquire or stop a bundled daemon. Remote targets never trigger a local daemon takeover.
- [ ] Preserve separate close, Quit and explicit Quit-and-stop behavior; a pre-existing managed Gateway stays healthy after ordinary shell Quit. Do not copy the entire Tauri supervisor or invent a second service registry.
- [ ] Test cold start, existing healthy service, foreign listener, unavailable service, recovery and target switch with temporary state and actual process identities. Do not touch installed services merely to make a test pass.

### Task 4 — Remaining OS parity and lifecycle on three platforms / 三平台宿主能力

**Files:** `desktop/nsh-shell/src/main.rs`, existing platform crates/helpers and new backend modules only as justified by Task1; reference `desktop/shell/src/{tray,hotkey,deeplink,notify,menu,webview_perms,perm_monitor}.rs`.

- [ ] Implement real platform entrypoints and supported window chrome, minimize/maximize, resident close/reopen, hidden Quit, tray/menu, hotkey conflict/disable/release, validated deep links, notifications/permission and placement.
- [ ] Preserve existing native capabilities that callers still use, including voice/file/permission helpers. The historical exclusion of a new NSH native-camera UI is not permission to break the shared Panel's existing native operations.
- [ ] Define notification feature-disable semantics if such a control is part of the new contract; OS permission denial alone does not prove no worker/cache/connection initialization. Avoid adding a new toggle merely to close a checklist.
- [ ] Verify G1–G7-style behavior on actual hosts, focused/unfocused and narrow windows, denial/conflict/reload and shutdown. Resolve multi-display Phase11 scope explicitly and keep any untested hardware item visible.

### Task 5 — Updater, packages, autostart and release builds / 更新、打包与分发

**Files:** `desktop/nsh-shell/macos/{Info.plist,bundle-local.sh}`, new release/Windows/Linux packaging paths selected after API proof; `desktop/shell/src/update.rs`, `desktop/shell/tauri*.json`, `.github/workflows/aleph-app-release.yml`, `scripts/stage_shell_placeholders.sh`, `justfile`, `src/bin/aleph-server/commands/update.rs`, `docs/reference/{PRODUCT_TOPOLOGY,RELEASE}.md`.

- [ ] Replace Tauri CLI/externalBin assumptions for full desktop, lite Panel and independent server products on all platforms; preserve server-only distribution and platform helpers. Local ad-hoc macOS signing is not release notarization/update proof.
- [ ] Preserve updater authenticity, version rules, concurrent-apply exclusion, managed/package-manager installation behavior, full/lite daemon ownership and recoverability. Do not silently reuse Tauri updater signatures with an incompatible verifier or add a new trust root without a decision.
- [ ] Retain autostart enable/disable, external-link allowlisting and supported URL bundle/registry declarations without changing system defaults before the authorized cutover.
- [ ] Produce build-only platform artifacts and inspect contents/launch behavior. Use the existing workflow's non-publishing path only when authorized for that external action; do not call `just release`, tag, publish or push as a build check.

### Task 6 — Freeze baseline and implement migration checks / 基线与自动验证

**Files:** Reuse `qa/nsh_panel_{launcher,matrix,parity}.py`, schemas, `qa/native_ipc/`, `qa/webview_compat/`, `tests/bridge_acceptance/`; propose `qa/phase11_nsh_cutover.py` and focused regression checks after reading repository QA guidance. Keep historical Phase9/10 receipt readers functional.

- [ ] Preserve a runnable Tauri baseline, exact source/binary/assets/lock identity and recoverable package before removal. Store large signed binaries locally or as approved build artifacts, not in Git.
- [ ] Check new contracts with meaningful negative/effect tests, including lost/reloaded documents, stale credential attempts, target switches, invalid origins, failure cleanup and no-op detection. Prefer type/lint/design prevention to more prose rules.
- [ ] Run relevant memory-gated package checks and fresh native comparative blocks serially on each platform. Interpret real footers and terminal facts; do not sum nested test summaries or reuse Phase10 observations as Phase11 runs.
- [ ] Add a machine-checkable gate evaluator that refuses cutover/removal for missing platform receipts, bad hashes, unresolved required failures or insufficient overlap duration; it must verify actual evidence references, not a hand-edited `all_pass: true` flag.

### Task 7 — Select NSH default, prove rollback and begin overlap / 默认切换与双轨启动

**Files:** `justfile`, CI/build launch routes, packaging metadata, Panel host selection, product docs and new `docs/superpowers/runs/phase11-dual-run/` journal/manifest paths.

- [ ] Only after P11-A–D and package/recovery checks pass, make NSH the default desktop in repository launch/build paths for all three platforms. Preserve deliberate Tauri rollback during overlap without dual singleton/URL authority.
- [ ] Exercise actual default app launch and rollback/reselect, state continuity and supported local/remote/full/lite scenarios. Execute the agreed installed-default cutover when its gates pass and applicable authorization covers it; prepare the exact signed packages and restoration path first. Do not ask again merely because the already-approved cutover touches associations.
- [ ] Record each platform's qualifying start date/UTC time, environment, code/build identities, default selection and operations. Count the common qualifying interval from the latest platform start, at least 14 calendar days with actual use and regression checks on each platform.
- [ ] Keep fixes and before/after witnesses; a material regression invalidates the affected qualification and its interval until corrected/requalified. Do not claim continuous quality over known failures or invent elapsed time to finish a session.

### Task 8 — Remove Tauri after the gates / 闸门通过后删除

**Files:** `desktop/shell/`, workspace `Cargo.toml` and `Cargo.lock`, Tauri-only frontend callsites/styles/baseline metadata, Tauri-only scripts and CI jobs, root `build.rs`, `justfile`, active source references found by Task1. Update `AGENTS.md` and `CLAUDE.md` together if their stack description changes.

- [ ] Verify P11-A–F receipts and exact 14-day minimum before any removal commit. This phase's target is complete removal, not leaving a permanent optional Tauri feature.
- [ ] Delete `desktop/shell` and exclusive manifests/capabilities/plugins/dependencies; regenerate the lock with Cargo rather than hand-pruning transitive crates. Keep shared Panel, platform helpers and non-Tauri consumers.
- [ ] Remove runtime `__TAURI*` probes, Tauri IPC calls, fake compatibility shims and active Tauri build/release routes. Historical docs/receipts, archived evidence and regression fixtures may keep clearly labeled historical strings; a raw text search is not the dependency proof.
- [ ] Census workspace membership (including the existing duplicate `desktop/nsh-shell` member entry), target-specific/dev/build dependencies and resolver graph. Remove genuine duplication during the migration without rewriting unrelated manifests.
- [ ] Re-run release/debug/native regressions and build-only full/lite/server products after deletion. If deletion breaks supported behavior, restore/fix within the worktree before declaring P11-G; do not hide the failure behind comments or skip flags.

### Task 9 — Final acceptance and local main handoff / 最终验收与 main 交付

**Files:** New Phase11 acceptance/source/default/deletion manifests, `docs/superpowers/runs/2026-11-21-phase11-closeout.md` (use actual completion date when created), `docs/reference/{FEATURE_LOCATOR,DESKTOP_SHELL,DESKTOP_BRIDGE,PRODUCT_TOPOLOGY,SUBSYSTEM_ROUTING,RELEASE,WINDOWS_RUNTIME,LINUX_DESKTOP}.md`, README variants and contributing/build docs where affected.

- [ ] Keep master Phases0–6 separate: native business UI/Plugin Runtime remain their own roadmap even after host retirement. State what remains untested and what was actually removed.
- [ ] Run identity, JSON/schema, local-link, default-asset and dependency checks; capture blocked commands and unfetched sources. Review the concrete final diff with ordered independent SPEC/QUALITY within authorized delegation.
- [ ] Complete P11-A–H with artifacts, not merely checked boxes. Commit verified implementation/removal to local `main` with conventional messages; no reset/worktree deletion/push implied.
- [ ] If only the observation period or a genuinely unavailable platform is pending, keep its Todo open and leave a durable exact handoff with next measurable action and date. Session persistence cannot manufacture 14 days or missing hardware evidence. / 不停在空泛计划；也不伪造完成。

## 6. Review focus and self-review / 审阅重点与自审

Review the highest-risk seams: native credential lease migration and failed-write rollback; updater signature/install ownership; remote/full/lite daemon distinctions; platform callback/task cleanup; default selection and rollback; real two-week qualification; active dependency removal rather than cosmetic string deletion. A required operation must name the production caller and executed handler and show the effect arrived. / 检查两端之外是否真正穿线。

**Planning self-review / 本计划自审:** Source locations and existing APIs above were inspected while planning; new files, official non-macOS support and future test outcomes are proposals, not completed facts. The operator retained both timing and platform gates. No implementation, physical topology test, 14-day dual run, installed-default change, release or Tauri deletion was performed by this planning task. Phase10's original failed verdict and revised scoped acceptance remain distinct.
