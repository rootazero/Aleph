# Panel Shell / descriptor projection next-stage implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: use `superpowers:executing-plans` and implement tasks sequentially in a fresh `panel-shell` worktree based on Terminal host acceptance commit `34a3c63bb`. One main writer; use Pi-routed read-only review for the finished branch. Do not merge, push, or change the default release.

## Goal / 目标

把当前“Terminal 特判 + 单个内置 descriptor”的 Panel 模块化 host 收口为可复用的 **host-owned descriptor projection → TabRegistry → route/active selection → PanelLoader → PanelSdk** 链路，为后续第二个 built-in Tab 迁移建立稳定边界。

本计划不迁移第二个真实 Tab，不做真实 Gateway smoke，不实现 community UI lifecycle。Terminal 是唯一产品模块回归对象；新增的第二 descriptor 只能是 test-only/inert contract，用来证明 host 不依赖 Terminal 特判。

## Current state / 当前状态

- Terminal production host/wire acceptance 已在 `34a3c63bb` 完成：真实 host WASM、Router、PanelShell、PanelSdk、版本化 terminal JS/WASM、loopback HTTP/WS fixture 均已验证。
- Component fixture 已覆盖 `/` 与 `/panel/` public base、PTY topics、input/resize/exit/spawn/attach、epoch 和 stale-result fence。
- Real Gateway/live PTY parity 仍为 BLOCKED；当前没有授权 disposable target/credential。
- `interfaces/webchat/src/panel/registry.rs` 已有 `TabRegistry`，但 `interfaces/webchat/src/panel/shell.rs` 仍直接构造单个 `aleph:terminal` descriptor；`interfaces/webchat/src/app.rs` 仍在 `MainContent` 中按 `PanelMode::Terminal` 特判进入 `PanelShell`。
- `panel-modular` 仍 opt-in，legacy path 仍为 rollback。

## Approaches / 方案比较

### A. 继续在 `PanelShell` 中添加更多 built-in 特判

实现成本最低，但每迁移一个 Tab 就增加一个 descriptor、route、feature 和 lifecycle 分支，无法形成稳定 projection 边界。淘汰。

### B. 先抽出 host-owned descriptor projection，再让 Shell/registry 消费统一列表（推荐）

将 built-in descriptor 组装、route key 映射和启用状态集中在 host-side projection 入口；`PanelShell` 只消费 `TabRegistry` 和 active selection，`PanelLoader` 负责 mount lifecycle。优点是 Terminal 行为可保持不变，下一 Tab 可复用；代价是需要明确 route/descriptor 关系和测试边界。

### C. 现在直接迁移第二个真实 Tab，并边迁移边抽象

能更快看到第二个模块，但会把当前 Terminal 特例和第二 Tab 的需求混在一起，失败时难以判断是 projection 还是第二 Tab 本身的问题。留作下一独立计划。

本阶段选择 **B**。

## Architecture decisions / 已决定的架构

1. `PanelTabDescriptor` 仍是 wire-facing descriptor；projection 只负责从 host-owned built-ins 生成经验证的 descriptor 集合，不成为 ExtensionManager 或 Capability registry 的第二权威。
2. `TabRegistry::new(...)` 是 built-in 初始化边界；built-in namespace 仍受保留，外部/plugin descriptor 的未来投影不能 shadow built-ins。
3. `PanelShell` 只读取 registry 的 enabled ordered rows 和 active key；descriptor label/icon/order/entry/status/permissions/trust 不在 view 中重新派生。
4. route 到 active key 的映射集中在一个 host helper/projection 中。未知 route、disabled/incompatible/failed descriptor 和非法 entry 必须 fail closed，并保留 Shell 的 error/retry surface。
5. 同一时刻只有一个 mounted Tab。切换、deactivate、retry 和 owner cleanup 必须释放旧 module handles/subscriptions/DOM roots；Terminal route leave 不调用 `pty.close`。
6. 本阶段不把 generic projection 接到 `ZahirFacade`、Capability runtime 或 community install lifecycle；只建立清楚的 host-owned seam。

## Task 1: Preflight and current contract map

- [ ] 从当前 git root 读取两级 `AGENTS.md`、本 prompt、本文、approved design、Terminal host plan/report、Terminal runtime/reference。
- [ ] 检查 `main`、`terminal-host` worktree 和 commit `34a3c63bb`；确认新 `panel-shell` worktree 不覆盖已有工作。
- [ ] 快速读取 graphify 指定的 `graphify-out/`；不重建全仓图谱。
- [ ] 查找实际存在的 unit/component/browser test skill；缺失时记录并遵循现有 Rust/Node/Playwright 约定。
- [ ] 绘制当前 descriptor 流：`PanelShell::builtin_descriptor` → `TabRegistry` → `PanelLoader` → `PanelSdk` → `app.rs` route，记录每个状态的 owner。
- [ ] 在开始编辑前形成一段中文 preflight：目标、当前证据边界、为何选择 projection-first、哪些内容明确不做。

## Task 2: Introduce one host-owned descriptor projection

- [ ] 在 `interfaces/webchat/src/panel/` 建立最小 projection 模块或等价入口，集中定义当前 built-in Terminal descriptor。
- [ ] 保留 `PANEL_TERMINAL_ENTRY` 与 `panel-tab-spike` 的现有构建行为；如果 spike feature 仍需兼容，projection 必须显式返回对应 descriptor，不能在 `PanelShell` view 内分叉。
- [ ] 让 projection 输出 `Vec<PanelTabDescriptor>` 或等价的 typed result，并在 host boundary 一次完成 validation；不要把 `unknown` 或未验证 descriptor 传入 loader。
- [ ] 为 descriptor 的 key、label、icon、order、entry、API major、trust、permissions、status/status_detail 建立单一测试入口。
- [ ] 明确 route mapping：当前 `/terminal` 对应 `aleph:terminal`；对于未投影的 route 返回 legacy/fallback 语义，不把任意 path 当作 module entry。
- [ ] 为 projection 增加 duplicate/invalid entry/API mismatch/status 失败测试；不新增第二份 registry。

**Acceptance:** `PanelShell` 不再直接定义 `PanelTabDescriptor` 字段；Terminal entry 与权限只在 projection 入口出现；现有 Terminal staged build 的 entry identity 不变。

## Task 3: Generalize PanelShell around registry state

- [ ] 修改 `interfaces/webchat/src/panel/shell.rs`，让 Shell 从 projection 初始化 registry，并从 registry rows 渲染 tab controls。
- [ ] 移除 `cfg!(feature = "panel-tab-spike")`/`cfg!(feature = "panel-modular")` 对 shell identity 和 descriptor construction 的耦合；feature 只影响 projection/build selection，不影响 Shell lifecycle code。
- [ ] 将 active key、close/deactivate、retry、loading/error/mounted status 的语义保持在 `PanelLoader`/host state；view 不再为 Terminal 单独推导逻辑。
- [ ] 保证 disabled/incompatible/failed rows 可查询并能显示诊断，但不进入 mount ordering；active removal 使用 registry 的 fallback 规则。
- [ ] 保证 repeated same-key navigation 不重复 mount；retry 只增加显式 attempt signal；owner cleanup 后 late future 不发布旧 status。
- [ ] 如果需要调整 `PanelLoader`，只修改经测试证明的 lifecycle owner；不要引入第二个 owner map 或 hidden global state。

**Acceptance:** 仅通过 registry/projection 增加一个 test-only inert descriptor，就能渲染第二个 tab row；切换到 disabled/invalid descriptor 不会调用 module mount；Terminal mount/retry/teardown 语义保持不变。

## Task 4: Remove route special-case duplication without widening scope

- [ ] 修改 `interfaces/webchat/src/app.rs` 的 `MainContent`，使模块化分支只负责判断当前 route 是否属于 host projection，再交给 `PanelShell`；不要为每个未来 Tab 增加 `if terminal` 分支。
- [ ] 保持 phone fallback 与 legacy `LegacyMainContent` 路径不变；本阶段不声称 phone/tablet modular parity。
- [ ] 将 route/key resolution 放在单一 helper/projection 中，添加 `/terminal`、未知 route、non-modular feature 的 Rust 单测或现有测试约定下的等价覆盖。
- [ ] 不把 `/panel/terminal` 当作 production route 证据；如果测试需要 public-base 变体，继续由 component fixture 覆盖，production artifact 的真实 root base 以 staged `index.html` 为准。

**Acceptance:** `rg` 不再发现多个 Terminal-only Shell entry branches；production host fixture 仍从 `/terminal` root route 启动，component fixture 仍通过 `/` 与 `/panel/`。

## Task 5: Regression and contract tests

- [ ] 保留并运行 `interfaces/webchat/tests/panel-terminal-protocol.test.mjs`。
- [ ] 保留并运行 `interfaces/webchat/tests/panel-terminal-host.test.mjs`；不得用手写 HTML、fake SDK 或 loopback fixture 替代 real host WASM。
- [ ] 补 Rust tests：projection descriptor identity、registry ordering/fallback、built-in shadowing、invalid/status fail-closed、same-key retry/one active mount。
- [ ] 如需浏览器扩展，只增加 test-owned inert descriptor/diagnostic assertion；不引入第二个真实 Tab 或新 runtime dependency。
- [ ] 每次 cargo 命令前从 worktree root 执行 `python qa/browser_live/check_memory.py`，cargo 从所属 package 目录运行；构建/测试串行。

Suggested checks:

```text
cd interfaces/webchat
node tests/panel-terminal-protocol.test.mjs
node tests/panel-terminal-host.test.mjs
node --test tests/panel-build.test.mjs

cd <worktree-root>
python qa/browser_live/check_memory.py
cargo fmt --all -- --check

cd interfaces/webchat
python ../../qa/browser_live/check_memory.py
cargo test --lib --no-default-features --features panel-modular
python ../../qa/browser_live/check_memory.py
cargo check --lib --no-default-features --features panel-modular --target wasm32-unknown-unknown
```

Adjust package/feature commands to the actual current workspace scripts; do not claim a command was run if it was unavailable.

## Task 6: Update evidence and references

- [ ] Add a short bilingual delivery section to `docs/reports/panel-terminal-tab-validation.md` or a new bounded delivery report. State that this phase generalizes host projection only; it does not add real Gateway evidence or migrate another Tab.
- [ ] Update `docs/reference/TERMINAL_RUNTIME.md`, `docs/reference/FEATURE_LOCATOR.md` §6.11, and the stale Terminal row in `docs/reference/SUBSYSTEM_ROUTING.md` only where current code/verified tests require it.
- [ ] Keep the three evidence verdicts separate: component fixture PASS, production-host/wire fixture PASS, real Gateway/live BLOCKED.
- [ ] Record the exact base commit, new branch, commands, scenario counts, unavailable checks and remaining next step. Do not write absolute machine paths in committed docs.

## Task 7: Independent review and handoff

- [ ] Run `git diff --check` and inspect the full diff including new files.
- [ ] Dispatch one Pi-routed read-only reviewer after implementation. Ask specifically about duplicate descriptor authority, route special cases, loader owner/epoch cleanup, built-in shadowing, and accidental scope expansion.
- [ ] Fix concrete findings and rerun affected focused checks. A timeout or failed review is not a pass; record it accurately.
- [ ] Commit with a conventional message such as `refactor(panel): generalize tab descriptor projection`.
- [ ] Verify the new worktree is clean. Do not merge, push, delete the previous `terminal-host` worktree, or flip the default release.

## Completion gates / 完成判据

| Gate | Required result |
|---|---|
| Projection authority | Terminal descriptor construction has one host-owned projection entry; Shell does not duplicate fields or maintain a second registry |
| Shell generality | Shell renders registry rows and owns generic mount/error/retry lifecycle; adding a test-only inert descriptor does not require Terminal-specific view branches |
| Route boundary | `/terminal` remains the production root route; unknown/unprojected routes fail closed or preserve legacy fallback according to current contract |
| Lifecycle | One active mount, explicit retry, cleanup of old handles, late futures fenced; UI teardown never calls `pty.close` |
| Regression | Component and production-host/wire fixtures pass; Terminal behavior is unchanged |
| Evidence | Docs state wire fixture PASS and real Gateway BLOCKED separately; no whole-Panel completion claim |
| Review/delivery | Applicable checks run and recorded; independent read-only review accounted; branch clean; no merge/push/default flip |

## Not completed by this plan / 本计划不完成

- Second real built-in Tab migration.
- Real Aleph Gateway/live PTY parity.
- Default `panel-modular` release flip or removal of legacy wiring.
- Community UI manifest/install/trust/update/uninstall lifecycle.
- Capability/ZahirFacade runtime mount.
- Phone/tablet parity, full accessibility/leak matrix, release rollback and whole-Panel size/memory claim.

## Next after this / 后续顺序

1. Select one concrete second built-in Tab using the generalized projection and define its own bounded migration/acceptance plan.
2. Run a separately authorized real Gateway smoke with a disposable project/session when such a target exists.
3. Only after multiple Tabs, live parity, failure/reconnect evidence and release rollback are independently accepted, write a default-flip plan.
