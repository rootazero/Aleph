# Panel Terminal production-host acceptance — restart prompt

实施 Panel UI 模块化的下一阶段：**Terminal 生产 host/SDK 确定性验收、证据修正与真实 Aleph Gateway 前置检查**。

本会话只执行已有设计下的有限验收收口，不重新 brainstorming，不重做前几阶段，不一次性迁移全部 Panel。开始时先用自己的话重述目标、当前证据边界和执行方式，然后给 preflight 结果。

## Read first / 先读

完整读取，以当前 `git rev-parse --show-toplevel` 为路径根：

1. 适用的上级约束、项目 `AGENTS.md`（保持与 `CLAUDE.md` 的双入口约束）。
2. `docs/superpowers/specs/2026-10-26-panel-tab-ui-capability-design.md`
3. `docs/superpowers/plans/2026-10-26-panel-terminal-tab-migration-plan.md`
4. **`docs/superpowers/plans/2026-10-27-panel-terminal-host-acceptance-plan.md`**（本轮执行计划）
5. `docs/reports/panel-terminal-tab-validation.md`
6. `docs/reference/TERMINAL_RUNTIME.md`、`DEVELOPMENT.md`、`SUBSYSTEM_ROUTING.md`、`REDLINES.md`，以及 `FEATURE_LOCATOR.md` 的 Terminal 入口/§6.11 和适用验证纪律。
7. 本 prompt；当前实际测试/build/生产 Shell/SDK 代码。

使用 `superpowers:executing-plans`，顺序推进计划 tasks。先通过当前安装的 graphify skill 读取 `graphify-out/` 现有图谱快速定位；不要以旧图谱替代当前源码，也不为本轮无关重建全仓图谱。测试前查找实际存在的匹配 test skill；不存在就记录并按项目 Rust/Node/Playwright 约定执行。

## Baseline / 已合并基线

- 上一开发分支 `terminal-live` **已经合入 `main`**，是 fast-forward 集成，集成点 `bf157d61a`。
- `fd6260168 test(panel): add terminal parity fixtures`
- `bf157d61a docs(panel): update terminal feature locator`
- 本 plan/prompt 的文档 commit 可能晚于上述点，先 `git log --oneline -5` 确认实际 HEAD。
- 默认分支是 `main`；不要虚构 `origin/dev`。
- 当前文档会话未 push、未切换默认发布构建。新会话仍没有 merge/push/default flip 授权。
- 旧 `terminal-live` worktree/branch 可能保留；只读检查，不删除或重用别人的工作。

从 clean 当前 main HEAD 创建新隔离工作区：

```text
git worktree add .worktrees/terminal-host -b terminal-host HEAD
```

已有同名 worktree 时先检查实际内容与状态，不 force/reset。后续所有实现/测试写入新 worktree；不要在主工作区实现。

## Why this next / 本轮要补的实际缺口

当前已经有 descriptor/loader/SDK、独立 terminal WASM、共享 `terminal-ui`、opt-in modular build、部分体积/去链接证据；默认 legacy 保留。

但上一轮验收的真实边界是：

- `panel-terminal-protocol.test.mjs` 加载真实 terminal JS/WASM，却通过 `pageHtml()` 手写导航和 `sdk`。**它没有启动 staged host WASM，也没有覆盖生产 `PanelShell → PanelSdk → DashboardState → WS`。**
- epoch 断言可能在 reconnect 之前已经满足；exit 只投递不存在的 session 并断言自己的 emitter；resize 可命中启动 resize；Canvas 只比较图片变化；spawn 没有实际 UI 断言。
- delayed attach/subscribe、disable/re-enable、missing/API-incompatible module 后真实路由/retry 未被完整证明，历史报告部分文字过宽。
- `opencode service status` 不存在只证明 opencode 不可用，不能推断 Aleph Gateway 一定不存在。真实 live 从未运行。
- 先前 Pi reviewer 超时无结果；全仓 pre-commit 第一次 `STATUS_DLL_INIT_FAILED (0xc0000142)`，第二次全量 clippy 超时，使用 hook 明确允许的 `--no-verify` 提交；不能声称独立 review 或全仓 hook 通过。

请保留这些历史事实，修正证据声明，补测试，不把已有绿色结果抹去或重新命名为 live。

## Execution / 执行要点

按计划 Task 1–5 完成：

1. **修正 component fixture**：受控 deferred 响应，真正选中 session 的 exit、spawn/attach、基线后 resize、新 epoch 下订阅/attach 与旧结果 fence；观察原生 Canvas 目标文字实际绘制，测试 instrumentation 必须调用原始 renderer 方法。
2. **新增 production-host/wire fixture**：最终 staged HTML/host JS/WASM/loader/terminal files 原样服务，由真实 Router/Shell/SDK 发 JSON-RPC。只在 loopback HTTP/WS server 边界模拟 Gateway，禁止手写替代 Shell/SDK。root 与 `/panel/`，包括 direct `/terminal`、`/panel/terminal`，真实 Chat/Settings 导航、失败诊断/retry、新连接恢复均有正数用例。
3. **修被测试证明的局部生产 bug**：owner/epoch/list/attach/subscribe/loader，保持 UI teardown 不关闭 PTY。没有实际缺陷就不做预防性重构。
4. **修构建执行纪律并测量**：build 脚本内部每个 cargo（包括 tree/版本调用）先 memory gate，从所属 package cwd 运行；同源码 legacy/modular inventory、font/CSS/Brotli、真实请求量、可测 timing、双重去链接；不可测量就明确 unavailable。
5. **真实 Aleph 前置检查**：通过 Aleph 自身 docs/QA 约定只读发现已有目标。只在明确已授权 disposable project/session 和 credential 安全注入都存在时运行 live；否则精确 BLOCKED 并继续独立 fixture/docs 工作。不得擅自启动或替换用户 Gateway。
6. **文档和审阅**：双语 report 按 component fixture / production-host wire fixture / real Gateway live 三层区分。校准 `TERMINAL_RUNTIME`、`FEATURE_LOCATOR §6.11`、`SUBSYSTEM_ROUTING` 过时入口。Pi 只读 whole-branch review，修真实 findings，再提交并报告。

运行/提交必须满足：

- 每次 cargo 前，从当前 worktree 根运行 `python qa/browser_live/check_memory.py`；仅 exit 0 放行；编译串行，cargo 从 package 目录执行。构建脚本的每个 cargo 子进程也适用。
- 提交前检查 hook 是否内部跑 cargo；不能违反上述 gate/cwd 约束。按 hook 明确允许的 bypass 处理冲突并记录，不能盲目再跑全仓 hook 造成无关长编译。
- 不修改 `src/harness/`、Capability runtime、ZahirFacade、PTY server business logic、community installation chain 或其他 Tabs。
- 不重启/替换/停用已有 Gateway，不操作已有用户 PTY/session，不把 credential 写入文件/报告/trace/screenshot。
- teardown 不调用 `pty.close`；live cleanup 仅 close 自己创建的 disposable PTY，必须有 cleanup 证据。
- `dist/` 测量副作用必须安全恢复；不要删除用户改动/缓存或 junction target。不要把旧 pilot/静态 HTTP 200 当 production-host 或 real Gateway 证据。
- generated glue 已可构建时，用 `ALEPH_PANEL_BRIDGE_GLUE` 尝试消除已有 loader skip；失败就保留准确原因和真实浏览器证据。
- Root 没有 `bun run check` script 就写 unavailable，使用实际 Rust/Node/Playwright 检查；禁止伪造通过。
- 单 writer 原生顺序执行，不派 writer subagent；独立审阅只读且经 Pi。review 超时/失败要记账，不当作完成。

## Completion / 完成判据

- component fixture 有效果级断言，且 production-host fixture 真正经过最终 host WASM、Shell、SDK、terminal WASM 和 WebSocket 协议，root/prefixed deployment 均正数通过。
- selected PTY 身份、screen/input、sequence gap/reconnect、迟到结果、错误隔离、route teardown 有可复查证据；无隐式 close/spawn。
- 同条件测量/manifest/双重去链接、真实请求与 timing 的实际可测范围、memory gate 和命令 ledger 已提交。
- real Gateway 是带 disposable cleanup 的 PASS 或有具体缺失条件的 BLOCKED；fixture 绝不替代 live。
- 文档准确、适用检查/review 结果齐全，worktree clean。
- wire fixture 通过但 live BLOCKED 时提交如 `test(panel): verify terminal host protocol`；只有 live 真通过才能使用 `verify terminal live parity`。
- 最终只报告 branch/worktree/commit、证据和剩余限制，**不 merge、不 push、不切换默认 release**。

## After this phase / 后续路线（本轮不实施）

Terminal 生产 host 验收只是第一个 Tab 模板。整体设计还需通用 Shell/权威 descriptor projection、其余 built-in Tabs 分批迁移、社区 UI manifest/安装/更新/卸载链、多端与完整性/可访问性/leak/发布回滚加固。此阶段完成不能宣称整个 Panel UI 模块化完成。

---

English handoff: execute the linked bounded plan in a fresh `terminal-host` worktree. Preserve the existing architecture and legacy rollback. Strengthen component assertions, load the final production host and terminal WASM through a test-owned HTTP/WS Gateway boundary, fix demonstrated lifecycle bugs, collect reproducible metrics, and distinguish wire-fixture evidence from real disposable-Gateway evidence. Do not restart user services, migrate more tabs, merge, push, or change the default release path.
