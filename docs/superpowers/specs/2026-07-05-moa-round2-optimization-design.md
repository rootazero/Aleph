# MoA 持续咨询第二轮优化设计 (Round 2: Fixes · Wiring · Enhancements · Polish)

- **日期**: 2026-07-05
- **状态**: 设计已确认（brainstorming 五节逐节确认）
- **前作**: [2026-07-05-moa-continuous-advisory-port-design.md](2026-07-05-moa-continuous-advisory-port-design.md)（第一轮移植，15 commits 合本地 main `6433a7ac1..185119c8e`，未推送未运行时 QA）
- **参考实现**: hermes-agent（`/Volumes/TBU4/Github/hermes-agent`）——本轮基于 59 项特性全量盘点 × Aleph 实现审计的交叉 gap analysis
- **任务目标**: 修复第一轮审计发现的 8 个缺陷、兑现 3 处未连线承诺、补齐 3 项增强（hermes 对齐+超越）、重构打磨与测试补齐，收尾做两轮首次运行时 QA

---

## 0. 一句话

第二轮不改 MoA 的架构本质（虚拟 `AiProvider` 门面），只做四类事：把审计出的缺陷修掉、把 spec 承诺过但没接上的线接上、把 hermes 还领先的三项能力补齐并超越、把代码和测试打磨到位。**全程零改 `src/harness/`（只调整 `trace.rs` 既有 MoA 变体的字段），所有改动收在 MoA 模块 + 既有消费面。**

## 1. 已确认的关键决定（用户澄清）

1. **增强范围** = E1 advisor prompt-cache + E4 多模态占位 + E3 select_model 选择器集成；**E2 聚合器流式后置**（Failover 链本身不透传 `as_http_provider`，生产链今天无 token 流式，MoA 被双层阻塞——先改 Failover 流式是跨切面大工程，独立立项）。
2. **渲染面** = 补 TUI（第一轮 "Task 9" 遗留），**phone/tablet 后置**（移动端根本没有 agent_trace 推理面，是平台级缺口非 MoA 特有，独立立项）。
3. **one-shot 语义** = 构建失败才回填（`try_build_for_run` 失败时原子回填 + 警告 trace；run 构建成功后的失败/取消不回填，保结构性零泄漏）。
4. **幽灵 agent** = 汇总层过滤/归类（`moa:*` 条目从 per-agent 列表剥离归入「MoA advisors」桶；原始 trace 逐顾问标签不动）。
5. **MoaTurnTrace** = 实现回放呈现（兑现第一轮 spec §8 承诺，`trace.by_runs` 时间线折叠呈现）。
6. **验证深度** = 逐任务单测门 + 收尾运行时 QA（重编 macOS App 替换 daemon 真跑，还清两轮运行时债）。
7. **三个架构决策点全取方案 A**：单 SDD 计划五阶段 / E1 在 MoA 模块内打标记 / E3 选择层集成不进 provider 注册表。

## 2. 事件 schema 兼容性前提

第一轮 15 commits 只在本地 main（未推送、未发版），四个 MoA 事件的 wire 字段**没有任何线上消费者**——本轮 B1-B4 的字段增删直接改，不需要兼容层。字段定稿后由 R4-1 RecordingSink 测试锁定。

## 3. 五阶段执行顺序（单 SDD 计划）

| 阶段 | 内容 | 理由 |
|------|------|------|
| P1 重构先行 | R1 拆 `process()` + R2 单遍化 + R3 不变量守卫 | 后续改动落在干净结构上，diff 更小 |
| P2 错误修复 | B1–B8 | 修复先于增强，增强不叠在已知缺陷上 |
| P3 功能连线 | W1 core tools、W2 TUI、W3 回放呈现 | 依赖 P2 事件 schema 定稿 |
| P4 功能增强 | E1、E4、E3 | 独立增量，各自可单独验证 |
| P5 测试与收口 | R4 测试补齐 + 文档 + 运行时 QA | 收口 |

## 4. P2 错误修复设计（8 项）

### B1 `advisor_count` 语义统一
现状：`MoaAggregating.advisor_count` = 咨询数，`MoaAdvisorSpend.advisor_count` = 有账单数（`provider.rs` spend_event 按过滤后 usages 计数）——消费者算均价会除错分母。
修复：两事件 `advisor_count` 统一为「本轮咨询的顾问数」；`MoaAdvisorSpend` 新增 `billed_count`（实际返回用量的顾问数）。

### B2 死错误通道接活
现状：fan-out 闭包返回 `(text, usage, Option<String>)` 三元组，错误成员被 `_err` 弃置——超时和 500 下游不可分辨。
修复：`MoaAdvisor` 事件加 `error: Option<String>`——成功为 `None`，失败/超时带结构化原因。面板/TUI 对带 error 的顾问块渲染警示样式。指导块 `[failed:]`/`[timeout after Ns]` 注记不变。

### B3 `MoaTurnTrace` 移到聚合器返回后发射
现状：trace 在聚合器调用前发射（`provider.rs:279` 先于 `:319`），永远缺聚合器输出——hermes trace 含聚合器完整 I/O（hermes #32/#33）。
修复：发射点移到聚合器返回后；补 `aggregator_output`（完整文本）与 `aggregator_status`（`ok` / `error: <摘要>`）。聚合器报错仍发射（顾问已实际运行计费，如实记录）；`process()` future 被取消时无 trace（可接受：顾问 spend 已由 per-advisor Metering 记录）。**`MoaAdvisorSpend` 维持在聚合器调用前发射**——开销在顾问返回时已真实发生，与聚合器成败无关。

### B4 cache-HIT 事件补光
现状：四事件只在 cache-MISS 发射；`user_turn` 节奏下迭代 2+ 是 HIT，面板整段变暗（观测撒谎：聚合器明明在跑）。
修复：`MoaAggregating` 加 `cached: bool`；cache-HIT 迭代发射 `MoaAggregating { cached: true }`。顾问块不重发（意见没变，重发是噪音）。面板渲染「◆ 聚合（沿用缓存顾问意见）」。

### B5 one-shot 构建失败回填
现状：`take_for_run` 在 run 构建瞬间无条件消费；`MoaProvider` 构建失败（preset 被删/slot 无法解析）时静默回退普通 provider，用户的一次性激活白烧且无感知。
修复：`try_build_for_run` 失败路径把已消费的 one-shot 原子回填 `session_moa_handle` + 发既有警告 trace（面板可见「MoA 未生效，已回退普通模型」）。run 构建成功后的失败/取消不回填。竞态安全由 `SessionRunRegistry` 每会话运行互斥保证。

### B6 幽灵 agent 汇总层归类
现状：per-advisor Metering 的合成 `agent_id="moa:<idx>:<provider>:<model>"` 进 per-agent 用量汇总 RPC，团队视图冒出假 agent。
修复：汇总 RPC 把 `agent_id` 前缀 `moa:` 的条目从 agent 列表剥离，归并为单一「MoA advisors」开销桶。原始 trace 逐顾问标签不动（审计粒度保留）。

### B7 channel `/moa` 前缀残留
修复：channel 路径拦截后把 `/moa ` 前缀从进入 LLM 的消息中剥掉，只留提示词本体（与 Panel/CLI 路径对齐）。

### B8 VESR 归因修正
修复：MoA active 时 routing 归因记录实际 serving 模型（聚合器）而非 override 模型——单点改 VESR 记录处，纯归因修正（metering 本就正确）。

## 5. P3 功能连线设计（3 项）

### W1 `moa` 进渐进披露核心集
把 `"moa"` 加入 `default_core_tools`（`src/config/types/tools.rs:162-173`）。理由：用户面激活开关应即时可调（R8 对话即管理面板），折叠态多一次 `get_tool_schema` 往返；成本约几百 token/轮，可承受。`is_enabled` 逃生舱语义不变。

### W2 TUI 四事件渲染
填掉 `interfaces/tui/src/tui/app/mod.rs:640-644` 的 stub，镜像 hermes CLI 形态（`cli.py:10810`）+ 面板既有语义：
- `moa_advisor` → 暗色折叠块 `◇ Advisor i/n — provider:model`（带 `error` 的显示 `[failed]` 警示色），正文走 TUI 既有 reasoning 预览样式；
- `moa_aggregating` → 状态行 `◆ aggregating (<聚合器 label>)`，`cached: true` 显示「(cached advice)」；
- `moa_advisor_spend` → 单行 `▫ advisors: N tokens / $X.XXXX`；
- `moa_turn_trace` 保持不渲染（persist-only，与面板一致）。

### W3 `MoaTurnTrace` 回放呈现
`trace_presentation.rs:481-484` 的 `moa_turn_trace` 分支从 `None` 改为折叠呈现——标题 `MoA turn trace — preset <名> (N advisors)`，展开后逐顾问折叠子块（输入 system+view 摘要 + 完整输出）+ 聚合器小节（注入后输入**摘要**、B3 新增的完整输出与状态）。重 payload 按既有 trace 呈现截断惯例截尾标注。效果：`save_traces=true` 会话在 `trace.by_runs` 回放中获得「为什么 MoA 这么建议」完整审计视图，零新 RPC。

## 6. P4 功能增强设计（3 项）

### E1 advisor prompt-cache 装饰（hermes #23 对齐）
顾问 payload 组装时打 `cache_control` 断点——advisor system prompt 一个 + advisory view **尾部最后 3 条消息**各一个（镜像 hermes `system_and_3` 布局）。标记走 Aleph 自己的 `ContentBlock::Text { cache_control }` 字段，Anthropic 协议适配器已映射为 `ephemeral`（`proto_impl.rs:73`），非 Anthropic 适配器天然忽略——**无条件打标，零按 provider 分支**。收益机制：advisory view 跨迭代 append-only，第 N+1 轮前缀命中第 N 轮缓存段；`per_iteration` 节奏下是最大成本项（hermes 实测无此项 Claude 顾问 0/1227 命中、11.5M token 重计费）。

### E4 多模态占位标记（超越 hermes #51）
`advisory_view.rs::text_of` 遇 `ContentBlock::Image` 渲染 `[image: <mime_type>]` 占位（hermes 静默丢弃）——顾问至少知道「这里有图」。`Json` 分支维持现状 + 补测试。Thinking 块继续丢弃（对齐 hermes）。

### E3 `select_model` 选择器集成（第一轮后置项兑现；hermes #47-49 映射）
- **形态**：选择层集成，"moa" 不进 provider 注册表（否决 hermes `moa://local` 假身份形态）。`select_model` 模型列表尾部附「Mixture of Agents」分组，列出全部 **enabled** preset（条目形如 `moa:<preset>`，带顾问阵容摘要）；disabled preset 不列出（对齐 hermes #47 enabled 门控，防禁用 preset 劫持）。
- **选中语义 = 选择器唯一槽位，互斥覆盖**：选 moa 条目 = 写 `session_moa_handle` sticky（等价 `moa on`）**并清除** `session_model_handle`；选普通模型 = 写 `session_model_handle` **并清除** MoA sticky。心智模型简单，不出现「选了模型却被 MoA 优先级压住」的困惑态。
- **`moa` 工具与选择器殊途同归**：写同一个 handle，`status` / Panel 选择器高亮同一真源。
- **Panel UI**：模型选择器下拉加「Mixture of Agents」分组，选中态显示 preset 名；数据经既有 models.list 类 RPC 附带，不开新 RPC。
- **行为变化说明**：现状「MoA > select_model」优先级共存，本节改互斥覆盖后正常路径下两者不再同时活跃；`runner_impl.rs` Step 3 优先级代码不动（防御性保留），互斥在写入端保证。

## 7. P1 重构 + P5 测试收口设计

### R1 `process()` 拆分（P1 先行）
619 行 `provider.rs` 按职责拆：fan-out 块（并行咨询+超时+降级）提炼为 `fan_out.rs`，事件发射块提炼为独立函数；`process()` 缩到「视图→缓存判定→fan-out→发事件→注指导→调聚合器」编排骨架（目标 <50 行）。B2/B3/B4 事件改动落在拆分后的发射函数。`display_name` 冗余字段清掉（由 `preset_name` 派生）。

### R2 热路径单遍化
`view_signature` 复用 `build_advisory_view` 已算出的文本（不再二次 `text_of` 全扫）；`truncate_tool_result` 从 3 次 `chars()` 扫描并为单遍。

> **修订注记（2026-07-05 实施）**: 实际交付为消除 2 次堆分配（String → 借用切片），保留 1 次全量 count + 2 次部分边界扫描。真·单遍需 ~TOOL_RESULT_BUDGET/2 的 ring buffer（≈16KB）反而引入分配，属更差工程。代码注释已如实声明。

### R3 缓存不变量守卫
签名缓存的「per-run 顺序调用」不变量写成模块级文档注释 + `debug_assert` 守卫（TOCTOU 显式化：若未来并发驱动同一实例，两个 MISS 会重复扇出）。

### R4 测试补齐
1. **RecordingSink 事件捕获测试**（第一轮头号 follow-up）：锁定 3 个 live 事件 wire 字段名 + 本轮新字段（`error`/`cached`/`billed_count`/`aggregator_output`/`aggregator_status`）+ cache-HIT 只发 `cached: true` 聚合事件；
2. `spend_event` 定价数学（token 求和、`billed_count` vs `advisor_count`）；
3. `save_traces` 门控（true 发射 / false 不发）；
4. `set_preset`/`delete_preset` happy path（mock ConfigPatcher + `store_moa_config` 热更新断言）；
5. `advisory_view` 的 `Json`/`Image` 分支 + E1 断点标记位置；
6. one-shot 构建失败回填 + E3 互斥覆盖语义；
7. Step 3-MoA 接线可行范围单测（mock deps 驱动 runner 构建路径）。

### 文档
FEATURE_LOCATOR §4.9 刷新（新字段/新连线/E3 集成）、MULTI_AGENT_SYSTEM.md MoA 节补第二轮内容、第一轮 spec 追加「第二轮修订」链接。

### 运行时 QA（两轮首次，收尾执行）
重编完整版 macOS App 替换 daemon（走既有刷新链，见 DESKTOP_SHELL.md），真实 preset 下验证：
- `/moa <提示词>` one-shot；`moa on` sticky 多轮；
- 面板 ◇/◆/▫ 事件 + `cached` 标注；
- 选择器 MoA 分组选中 / 切回普通模型（互斥覆盖生效）；
- `save_traces=true` 后 `trace.by_runs` 回放看到完整 turn trace；
- 用量汇总无幽灵 agent；
- TUI 目视一次三事件渲染。

### 验证门（沿第一轮标准）
逐任务定向单测 + 收口一次 `cargo check --lib` + 0 警告；极度节制全量 cargo。

## 8. 范围外（明确不做）

- **E2 聚合器 token 流式**：先决条件是 Failover 链透传流式（跨切面大工程），独立立项。
- **phone/tablet MoA 渲染**：移动端 agent_trace 推理面是平台级缺口，独立立项。
- K-of-N 竞速提前聚合（沿第一轮）。
- 频道端（Telegram 等）顾问块渲染（沿第一轮）。
- hermes legacy 标记编解码 / 扁平配置兼容视图（沿第一轮）。
- `MoaAdvisor` 事件的逐 token 流式预览。

## 9. 红线合规

- **R10**：`src/harness/` 只动 `trace.rs` 单文件、且只做既有 MoA 变体的字段级调整（事件载体非认知，字段随 schema 定稿属第一轮变体的自然收尾）；不新增文件、不新增变体、12 文件预算零增长；
- **R3/R7**：无新依赖、无规则引擎；E1 打标是静态装饰非按内容分支；
- **R8**：E3 选择器与 `moa` 工具双入口同一真源，对话即配置；
- **P6 YAGNI**：`display_name` 冗余清除；死错误通道要么接活（B2 已接活）要么删——本轮接活。

## 10. 关键锚点（实施计划引用）

- 门面与 fan-out：`src/providers/moa/provider.rs`（`process` `:178-321`、`try_build_for_run` `:59-133`、`spend_event` `:143-174`）
- 视图与签名：`src/providers/moa/advisory_view.rs`（`text_of` `:51`、`view_signature` `:147-159`、`truncate_tool_result` `:27-40`）
- one-shot handle：`src/providers/session_moa_handle.rs::take_for_run` `:54-61`
- Step 3 接线：`src/orchestrator/harness_bridge/runner_impl.rs:123-155`
- 事件定义：`src/harness/trace.rs:129-153` + 协议镜像 `shared/protocol/src/events.rs:388-442` + 呈现 `trace_presentation.rs:451-484`
- TUI stub：`interfaces/tui/src/tui/app/mod.rs:640-644`
- 面板渲染：`interfaces/webchat/src/platform/wide/views/chat/events.rs:200-231`
- 渐进披露核心集：`src/config/types/tools.rs:162-173` + `progressive_disclosure.rs:74-92`
- 用量汇总：`resilience/database/traces.rs`（ProviderUsage 按 agent_id 聚合处）
- channel 拦截：`src/gateway/execution_engine/execute.rs:239-254` + `slash_command.rs:147-164`
- select_model 范式：`src/builtin_tools/select_model.rs` + `src/providers/session_model_handle.rs`
- cache_control 映射：`src/providers/protocols/anthropic/proto_impl.rs:73,145`
- hermes 行为契约：59 项特性盘点（本轮 brainstorm 会话内 Explore 报告）；关键项 #13-15 流式、#16-19 成本、#23-24 缓存、#26 节奏签名、#32-35 trace、#47-49 选择器、#51 多模态
