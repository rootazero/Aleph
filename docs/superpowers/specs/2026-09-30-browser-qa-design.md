# browser_qa 设计：单调用诊断裁决（C3）

> 状态：设计已逐节获批（2026-09-30），待实施。Round 2 浏览器自动化重构第三块。前作：C1（网络 mock，merge `63a2196b9`）、C2（会话录制，merge `3c488da53`）。
> 执行纪律、验证集、文件边界见实施计划（`docs/superpowers/plans/2026-09-30-browser-qa.md`，writing-plans 阶段产物）。

## §1 意图与硬约束

**意图**：交付 `browser_qa`——**单调用诊断裁决**工具。模型完成浏览器任务后自问自答「这页面现在健康吗 / 我的操作达到预期了吗」，一次调用拿回裁决+证据，而不是手工拼装 4-5 次散件调用（现状：`browser_wait_for`+`browser_console`+`browser_snapshot`+`browser_evaluate` 手工组合，无人做基线减法、失败分级、统一证据包）。**自验证优先**：形状为「动作后的状态确认」优化；**测试编写兼容**：verdict 结构化到可当测试断言用。三个 Round 1/2 挂号项同轮处理：① tab 死亡检测器做掉；②③（playwright-cli 的 active/targetId 不可观测）显式降级。

**硬约束**：
1. **方案 A（已裁定）**：QA 是编排+裁决，住工具层（`builtin_tools/browser_tools/qa.rs`），只编排现有 trait 动词 + 纯本地逻辑；**零新引擎原语**。死亡检测器是 backend 独立组件，与 QA 工具解耦，QA 只是其受益者
2. **仅 attached（已裁定）**：作用于既有 tab；不新开页面、不清缓冲。基线减法适配 attached 语义（qa 开始拍基线 → 终读只计新出现）
3. **诚实三角**：检查不可用报 **skip**（归入 unverified，注明原因）；证据不足报 **unverified**；只有实测不符才报 **fail**。绝不让 skip 冒充 pass
4. **R39**：无动词则无能力行。新行 `error_events` 挂在 `browser_qa` 动词落地之后；obscura 的 `Runtime.exceptionThrown` 可靠性未知（`Page.javascriptDialogOpening` 静默缺失有前科，capability.rs:168-178），caps 行探测是交付条件
5. **R9 字节纪律**：DESCRIPTION ≤80B；catalog ceiling 棘轮现 116,150B
6. **唯一 CDP 真源**：死亡检测器用 aleph-cdp 已有 `Target` 域包装（methods/target.rs 5 方法齐全含 `set_discover_targets`），零新 CDP 代码
7. **证据纪律**：失败证据复用现有件（截图走既有 screenshot 动词+artifact 链），不造新证据管道

## §2 组件与数据流

三个组件，一个数据流：

**① `qa.rs` 编排器（工具层，唯一新工具逻辑）**——qa 管线全复用现有动词：

```
开始 → 拍基线（读 console 缓冲 + network 环当前尾部）
     → 等待期望（expected_text/expected_selector/gone_selector，委托 wait_for 轮询机械——gone_selector 需加 `selector_gone` 枚举变体，同一机械非新原语；有界 5s）
     → 诊断沉淀（固定 150ms——参考实现 job.js 同款，让异步错误落缓冲）
     → 终读（console 新增行 + network 新增行 + exceptionThrown 折入的 [error] 行，cdp_backend/events.rs:106-114）
     → 基线减法（只有「基线后新出现」的计入）
     → 失败分级（favicon/analytics/sourcemap 类低影响 → warning；文档/脚本/XHR 失败 → actionable fail）
     → 组装 verdict
```

**② 基线减法 + 失败分级（纯本地，零引擎依赖，可单测到牙齿）**
- 减法按**计数对齐**：以规范化行文本为指纹，基线中出现过 N 次的行，终读中前 N 次出现不计为新增（参考实现 job.js:152-175 语义移植为 Rust 纯函数）
- 分级器是**数据不是逻辑**：substring 匹配表（R8），常量数组

**③ tab 死亡检测器（cdp_backend 独立组件）**
- 事件泵 arm `Target.targetDestroyed`（`targetCreated` 的 popup 收养先例在 events.rs:254,279；aleph-cdp 事件广播永不关闭已记档——死亡检测的真实通道是事件内容而非流结束）
- 死亡事件 → TabTable 折叠；`tab_registry::resolve_identity`（tab_registry.rs:246-265）的 TabGone 裁决从「调用方喂 `live_target_ids` 主动枚举」升级为事件推送
- 效果：QA（及任何动词）对死 tab **立即答 TabGone** 而非等命令超时；顺带关 C2 残余（录制中 tab 死亡由事件触发收尾，不再只靠 socket 级检测）

**诚实点**：attached 模式的基线减法不可能完美——qa 窗口内并发页面活动产生的同指纹新行会被基线额度吃掉而漏报。对策已内建：计数对齐 + `unverified[]` 显式列出「与基线匹配、无法证实新旧」的行。

## §3 wire 面

`browser_qa`（第 28 个工具），**无 action 字段**（单动词），一次调用一次裁决：

```
browser_qa {
  tab_id?,                      // 缺省 = 当前活动 tab
  expected_text?:    string | string[],   // 有界可见文本断言（上限 6000 文本节点，参考实现同款）
  expected_selector?: string,             // CSS 选择器存在性断言
  gone_selector?:     string,             // 选择器消失断言（自验证高频：「spinner 没了」）；WaitCondition 现无此变体——加 `selector_gone` 变体（复用同一 evaluate 轮询机械，types.rs:76-90 的扩枚举，不是新原语）
  check_console?:    bool,   // 默认 true
  check_errors?:     bool,   // 默认 true（exceptionThrown 折入行）
  check_network?:    bool,   // 默认 true（失败请求分级）
  screenshot?:       bool,   // 默认 false；true 时失败即附证据截图（既有 screenshot 动词+artifact 链）
  timeout_ms?:       number  // 期望等待上限，默认 5000，上限 30000
}
→ {
  passed: bool,               // 充要条件：failed_checks 空 且 unverified 空
  failed_checks: [{check, detail}],    // 实测不符
  warnings:      [{check, detail}],    // 低影响失败
  unverified:    [{check, detail}],    // 证据不足 / driver 降级 skip（注明原因）
  summary: string,                     // 一句话裁决；unverified 非空时必须说清「通过 X 项、Y 项无法证实」
  evidence: { screenshot_path?, console_tail, network_tail }
}
```

关键 wire 决策：
1. **三态分离落进 schema**：三数组并列；skip/unverified 永不计 pass
2. **断言失败 ≠ 工具错误**：期望等待超时进 `failed_checks`，工具返回成功 + `passed=false`——模型要的是完整证据包不是裸 error
3. **approval gate 不进**：QA 是纯读操作；screenshot 证据复用 screenshot 动词自身审批姿态，QA 不加第二层
4. DESCRIPTION（80B，实测顶格）：`"Assert page health: text/selector expectations plus console/network/error checks"`

## §4 能力台账与错误面

**新增一行**（R39：动词落地才有名分）：

| 行 | chromium | obscura |
|---|---|---|
| `error_events` | Supported（console/evaluate 已实测，同泵推断） | **NOT_PROBED → caps 行探测**（触发页面异常 → 断言 `[error]` 行到达） |

其余检查不需要新行：wait_for 委托现有动词；console/network 读既有环缓冲；减法/分级是纯本地逻辑。playwright-cli 有 console/wait/文本能力，QA 大部分检查天然可用；缺的维度进 `unverified` 注明「driver 不支持」。

**错误面**：
- 断言失败不是工具错误（§3.2）；只有引擎/驱动级故障走 `backend_error_text` 咽喉（recovery 契约+nextActions 免费获得）
- **TabGone 立即答**：死亡检测器落地后，QA 作用于死 tab → `BrowserError::TabGone` 走咽喉
- **不照搬**参考实现的 tool_result 翻转（qa-failure → tool error，TOOL_CONTRACT.md:601）：verdict 是数据，passed=false 自带信号，翻转会丢三数组结构。**与参考的有意分歧，登记在此**
- **注册是九处不是七处**（C2 全量测试抓到 T3 漏了 `approval_wiring_census::SOURCES` + `tabs.rs::BROWSER_TOOL_SOURCES`）——实施计划清单写全九处

## §5 刻意不做

- URL 模式（不开新页面、不清缓冲——已裁定 attached-only）
- exec DSL 加 qa 步骤（QA 失败要产证据包+交互决策，塞批内违反 DSL 逐步过咽喉纪律；后续增量再议）
- tool_result 翻转（§4 有意分歧）
- 视觉回归/截图 diff（证据截图只留证，不做像素比对——独立重功能）
- 性能断言（load 耗时、内存——无现状需求）
- ②③的驱动层修复（playwright-cli 协议限制，QA 只观测不修）
- qa 常驻 CI stage（沿用 caps 行先例，不新建 stage）

## §6 执行形状

worktree 隔离（`Aleph-browser-r3` / branch `browser-native-r3`，从最新 main 切）：

| 任务 | 内容 | 依赖 |
|---|---|---|
| T1 | tab 死亡检测器：事件泵 arm `targetDestroyed` + TabTable 折叠 + TabGone 即时裁决；关 C2 残余（录制中 tab 死亡事件触发收尾） | 无 |
| T2 | QA 本地逻辑：基线减法（计数对齐纯函数）+ 失败分级器（数据表驱动） | 无 |
| T3 | `browser_qa` 工具：编排管线 + 九处注册 + verdict schema + `error_events` 台账行（obscura NOT_PROBED） | T1+T2 |
| T4 | 探针+文档+验证：obscura `exceptionThrown` caps 行 + FL §3.12 C3 段 + 挂号项关账 + 七条全量验证集 | T3 |

T1 与 T2 可并行（backend 层 vs 纯函数，零交集），T3 汇合，T4 收尾。沿袭既有纪律：TDD + 每条守卫先证伪、内存守卫、英文 commit、合并前七条全量（基线 20,324 passed / 6 既存红 + pty flaky）。
