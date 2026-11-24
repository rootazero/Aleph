# Restart Prompt — Panel Terminal live parity and final validation

> 用法：下一会话从当前主仓库打开，粘贴下方提示词。本文件是执行入口，不在本会话自动开始下一阶段。 / Paste the prompt below into the next session; this file does not start the next phase automatically.

```text
实施 Aleph Panel Terminal Tab 的下一阶段：真实 Gateway/live parity 验证与最终验收收口。

先完整读取：
1. D:/Workspace/Aleph/AGENTS.md，以及适用的上级项目约束
2. docs/superpowers/specs/2026-10-26-panel-tab-ui-capability-design.md
3. docs/superpowers/plans/2026-10-26-panel-terminal-tab-migration-plan.md
4. docs/reports/panel-terminal-tab-validation.md
5. docs/reference/TERMINAL_RUNTIME.md、docs/reference/DEVELOPMENT.md、docs/reference/SUBSYSTEM_ROUTING.md、docs/reference/REDLINES.md
6. 本文件

目标是完成原计划 Task 6 的可执行部分：用真实最终资产链做确定性浏览器协议 fixture，若环境提供已授权的 disposable Gateway，则完成真实 PTY/live smoke；补齐同条件旧构建与模块化构建的测量、运行时生命周期说明和双重去链接证据。不要重新实现 Task 1–5，也不要重新 brainstorming。

## 当前基线

- 当前主分支已合并本阶段实现，合并提交是 `63623f160 merge(panel): integrate terminal tab migration`；本 prompt 和验收报告可能在该提交之后新增，先用 `git log --oneline -5` 确认实际 HEAD。
- 本阶段原实现提交包括：
  - `22e136293 feat(panel): add standalone terminal wasm tab`
  - `940ca3cad feat(panel): integrate dynamic terminal navigation`
  - `6916ab514 feat(panel): add modular terminal build pipeline`
- 默认 legacy 构建仍是默认路径；`panel-modular` 是 opt-in；没有切换默认发布构建。
- 主仓库实际默认分支是 `main`；本机此前确认没有 `origin/dev`，不要虚构该远端。
- 上一实现 worktree 已清理。本阶段必须重新创建独立 worktree，例如从当前 HEAD 创建短分支 `terminal-live` 和 `.worktrees/terminal-live`；不要直接在主工作区实现。
- 当前主分支未推送，领先 `origin/main` 11 个提交。不要在本阶段 push、merge 或切换默认发布，除非用户在新会话明确授权。

## 已有实现与证据

- `interfaces/webchat/tabs/terminal-ui/` 是共享真实 Terminal UI。
- `interfaces/webchat/tabs/terminal/` 是独立 `panel-tab-terminal` WASM 入口。
- `interfaces/webchat/scripts/build-panel-tabs.mjs` 构建 host + terminal wasm-release、wasm-bindgen、wasm-opt、Tailwind、manifest 和版本化 entry。
- `interfaces/webchat/scripts/panel-tabs-report.mjs` 生成同构建资产报告。
- `interfaces/webchat/tests/panel-build.test.mjs` 已验证 manifest、入口、依赖与资产完整性。
- `just wasm-modular` 已通过；当前模块入口示例为 `./tabs/terminal/cb296d71a70d9eaa/terminal.js`。
- standalone terminal native tests 5/5、WASM check、WASM clippy `-D warnings` 已通过。
- 静态 Playwright fixture 1/1 已通过：host/loader/CSS/terminal JS/WASM 均 HTTP 200，无 console error 和 failed request。
- `node --test tests/panel_loader.test.mjs` 为 8 passed / 1 skipped；skip 是缺少 generated feature glue，不能冒充已测。
- `docs/reports/panel-terminal-tab-validation.md` 已记录已有证据和 live 阻塞，不要抹掉历史限制。

## 执行边界

- 这是顺序收口任务。不要派 writer subagent。只有真正独立 review、专属 review 环境、真并行交付物或大日志隔离才可以使用 subagent；如需独立 review，必须只读并经 Pi 路由。
- 每次 cargo 命令之前，从仓库根执行 `python qa/browser_live/check_memory.py`；只有 exit 0 才运行 cargo。测试从所属 package 目录执行，编译串行。
- 不要运行 `cargo` 于根目录。不要用 ignored fixture、pilot artifact 或静态 HTTP 200 冒充 real Gateway evidence。
- 不修改 `src/harness/`、Capability runtime、ZahirFacade、PTY server business logic、community installation chain 或其他 Tab migration。
- UI teardown 只能释放 DOM、Owner、订阅、RAF/observer 和 module handles；不得因为卸载/路由离开/disable 调用 `pty.close`。只有本次测试明确创建的 disposable PTY 才能在测试末尾显式 close。
- 不重启、替换、停用用户现有 Gateway，不干扰已有 PTY/session，不把任何 credential 写入仓库、报告或静态资产。
- 默认 legacy build、`dist/` 和已提交的 release 入口保持可回退；不要把 modular 直接改成默认发布。

## Preflight

1. 检查 git root、status、worktrees、branch、HEAD，并确认当前主工作区没有未授权改动。
2. 从当前 HEAD 创建独立 worktree：
   `git worktree add .worktrees/terminal-live -b terminal-live HEAD`
3. 在新 worktree 确认当前实现可读取；不要从历史 spike commit 重新开始。
4. 更新 todo/ledger，记录每个实际命令、计数、skip 和 blocked 原因。
5. 若需要测试规范，先读取 `.opencode/skills` 下适用的 unit-tests、component-tests 或 app-e2e-tests skill。

## Task A — 完成确定性浏览器协议 fixture

在最终 `.panel-tabs` staging 资产上实现真实协议路径 fixture，文件放在 `interfaces/webchat/tests/`，不要只测 `mount()` 是否被调用。

至少覆盖：

- Chat/startup 不请求 terminal module、不 mount terminal。
- 首次进入 `/terminal` 时只加载一次版本化 terminal JS/WASM，sidebar 与 Canvas 两个根均可用。
- 从 Terminal 切到 Chat/Settings 再返回：module 可复用或重新 mount，但不产生重复 listeners；PTY 仍存在。
- `pty.list` 权限/transport failure：显示真实拒绝或错误文案，不隐式 `pty.spawn`。
- spawn、attach、input、resize 的 JSON/字节边界；至少用一个 Unicode marker 和 bracketed-paste payload。
- screen/exit 事件按 exact topics 投递，sequence gap 触发已有 attach recovery，不生成假成功。
- connection epoch 变化后，旧异步 list/attach/subscribe 结果不能写回已销毁 UI；新 epoch 可以恢复同一 selected PTY。
- quick switch、disable/re-enable、width change：DOM 不被 late result 修改，roots 清空/重建幂等，UI teardown 不调用 `pty.close`。
- 缺失或 API 不兼容的 module：错误只影响 Terminal，Chat/Settings 仍可用，并有 retry/navigation path。
- root deployment 与 `/panel/` prefixed deployment 都使用最终 staged files 和相对 public base。

fixture 要断言可见屏幕/内容与 host RPC 事件；不能只断言函数被调用。明确报告这是 protocol-fixture evidence。

## Task B — 检查真实 Gateway 前置条件并尝试 live smoke

只做只读前置检查：

- `opencode service status`
- 若可用，遵循 `AGENTS.md` 的 `bun run dev:live` 方式复用当前 elected service、私有 local credential 和 dev TUI storage channel；不要自行启动替代服务。
- 若命令不存在、服务不在、没有明确可用的 disposable project/session 或 credential 不可安全注入，则立即将 live 结果记录为 `BLOCKED`，不要安装、重启、替换或猜测服务。

如果存在已授权的 disposable Gateway，执行最小 smoke：

1. 创建本次测试唯一的 disposable terminal session。
2. 通过真实 Panel host → SDK → terminal module → Gateway/WS 发送 echo marker。
3. 在真实 Canvas/屏幕中观察 marker；验证 Unicode、bracketed paste 和 resize。
4. 切到 Chat 再回 Terminal，确认同一 PTY/session 仍可 attach。
5. 模拟或等待一次 transport drop/reconnect，确认新 connection epoch 与事件订阅恢复，旧结果被 fence。
6. 只关闭本次测试创建的 disposable session，并记录 cleanup 证据。

真实 Gateway 不可用时，fixture 可以通过，但 production acceptance 必须明确写 `BLOCKED`；不能把 fixture 改名为 live。

## Task C — 同条件构建和测量

使用同一个 source revision、同一个 `wasm-release` profile、wasm-bindgen/wasm-opt 版本、CSS、font 和 Brotli 设置：

- 生成旧 default legacy 构建和 modular candidate；如果 `just wasm` 写入 tracked `interfaces/webchat/dist/`，先保存并在验证后恢复无关产物，检查最终 git diff，不要把测试副作用混入提交。
- 分别记录 host-only、terminal module、total distribution、first-page transferred bytes。
- 记录 browser load/instantiate timing（如果只有静态构建则明确写 unavailable），不要编造 RSS 或 OS memory reclamation。
- 保留 candidate manifest 中真实 descriptor/entry 的匹配证据。
- 给出两份独立去链接证据：
  1. modular host 的 `cargo tree -p aleph-panel --no-default-features --features panel-modular --target wasm32-unknown-unknown` 不含 `aleph-terminal-ui`；
  2. artifact/source inspection 证明 terminal-specific renderer code/data 出现在 terminal artifact，而不是 host。考虑 release stripping，不能只依赖 symbol name。
- 明确其他尚未迁移的 Tabs 仍在 host 中，不能宣称整个 Panel 已轻量化。

## Task D — 文档、验证与提交

- 更新 `docs/reference/TERMINAL_RUNTIME.md`：明确 UI module teardown、PTY/session lifetime、reconnect epoch 和 `pty.close` 的授权边界。
- 更新 `docs/reports/panel-terminal-tab-validation.md`：加入真实 fixture、同条件测量、工具版本、精确命令/计数、live 结果、skip/block 原因和 legacy rollback 命令。
- 默认与模块化 Panel 测试均执行正数用例；terminal-ui、terminal module native/WASM check、focused clippy、`cargo fmt --all -- --check`、loader/build tests、fixture tests 按适用范围执行。
- `bun run check` 在当前 `package.json` 没有对应脚本时记录 unavailable，不要伪造成功；按仓库实际 Rust/Node/Playwright 命令验证。
- 每条 cargo 命令都保留 memory gate 证据。
- 运行 independent whole-branch read-only review（仅在确有独立审阅价值时派 Pi reviewer）；根据 review 修复真实问题。
- 若 live Gateway 通过，提交：`test(panel): verify dynamic terminal parity`。
- 若 live Gateway BLOCKED，仍可提交确定性 fixture、测量和文档，但提交信息应准确描述实际内容，例如 `test(panel): add terminal parity fixtures`；不得声称 live parity 完成。
- 最终不要 merge/push/default release flip；只报告 branch/worktree、commit、clean status、证据和剩余阻塞，等待用户下一步授权。

## Completion criteria

完成必须同时满足：

- deterministic fixture 覆盖真实 SDK/JS glue/module/Canvas 协议路径；
- legacy 与 modular 的同条件测量和去链接证据已提交；
- live Gateway 要么真实通过并有 disposable cleanup 证据，要么明确 BLOCKED 且未冒充通过；
- runtime 文档和 bilingual validation report 已更新；
- 默认 legacy 可回退，worktree clean，未 push/merge/default flip。

先输出简短的验收目标和 preflight 结果，再开始 Task A；不要重复上一阶段的设计讨论。
```
