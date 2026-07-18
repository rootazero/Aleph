# 渐进式工具披露 (Progressive Tool Disclosure) — 设计

- **日期**: 2026-07-03
- **状态**: 设计已批准，待写实施计划 (writing-plans)
- **关联红线**: R7 (LLM 主权) / R8 (工具即一切) / R9 (智慧在 prompt) / R10 (薄 Harness) — R10 第 2 不已于本次同步修订澄清
- **关联采纳条款**: A1 (自有 Context Window) / A3 (状态可重建，趋向纯 reducer)

---

## ⚠️ 设计修订（调查后定稿，2026-07-03）

初稿 §4–§6/§9 基于"从零造"假设。**读码调查（3 条只读探针 + 真实抓取数据）后大幅简化**——下文修订段**取代**初稿中冲突的部分（§1/§2/§3/§7 不变）。

**关键发现**：
- **schema 占工具 token 的 80%**（32,297/40,195），name+desc 仅 7,214 → 只压 schema 即可拿下大头。
- `ScopedToolService`（每轮新建）有现成扩展点 **`ToolDefinitionRewriter`**（`src/tools/scoped/traits.rs:32`，`fn rewrite(&self, def: &mut ToolDefinition)`），list()/metadata_schema() 自动应用——**过滤/压缩工具的天然落点，零 harness 改动**。
- system prompt **每 run 静态装配一次、看不到工具/transcript** → 动态目录/loaded-set 那套在此路径上行不通，**也不再需要**。
- meta-tools（`get_tool_schema` 等）已建但因 `config.tool_catalog=None` 未接线（死代码）；`ToolCatalog` 生产已构造。**但接线它需动 boot 高危区**——改用**每请求注册的极简 `get_tool_schema` LoopTool**（持全量 schema 快照）规避。

**修订后机制（Option X）**：非核心工具的 `input_schema` 在 rewriter 里压成开放占位 `{"type":"object","additionalProperties":true}` + description 追加"call `get_tool_schema('<name>')` for params"提示；工具**始终留在 `tools` 数组**（name+desc 就是目录，无需独立 block）；模型 `get_tool_schema` 取回参数后直接调用（开放 schema，API 无障碍）。

**溶解掉的初稿组件**：loaded-set 从 transcript 归约 ❌、独立工具目录 block + 注入点 ❌、think.rs 每轮注入 ❌、接线 ToolCatalog 动 boot ❌。

**验证数字（真实抓取算）**：Option X（留 desc）50.6K → **~22K（省 56%）**；Option X2（desc 截首句，config 旋钮）→ **~19.4K（省 62%）**。默认 X。

**修订后落点（6 处，全在 `src/config/` + `src/tools/` + `src/gateway/execution_engine/` + `src/thinker/`，零 boot/executor 改动）**：
1. `[tools] core: Vec<String>` + `truncate_tool_descriptions: bool`（`config/types/tools.rs`；`core=["*"]` 或空 = 逃生舱回旧行为）
2. `ProgressiveDisclosureRewriter`（新 `ToolDefinitionRewriter`）
3. 极简 `get_tool_schema` LoopTool（每请求持全量 schema 快照）
4. run_loop 接线：注册 `get_tool_schema` + `build_request_tool_service` 挂 rewriter + 透传 core config
5. prompt 指引层（说明折叠 schema 模式，复刻 `DEFERRED_LOADING_GUIDANCE`）
6. token 门测试

详见实施计划 `docs/superpowers/plans/2026-07-03-progressive-tool-disclosure.md`。

---

## 1. 背景与动机

单轮"你好"实测（ground truth = session `s79` transcript + 真实抓取的 provider 请求体 `req_03.json`）：

| 组成 | ~tokens | 占比 |
|------|---------|------|
| tools schema (168 个) | ~40,200 | 79.5% |
| system prompt | ~10,300 | 20.3% |
| user message | ~100 | 0.2% |
| **合计** | **~50,612** | 100% |

- 168 个工具的**完整 schema 每轮硬发**；主 agent 走 kimi 代理（`api.kimi.com/coding`，Custom endpoint，`supports_cache_control=false`）→ **每轮全额重算，零缓存**。
- 纯"无损裁剪"天花板已被证明仅 ~0.7%（skills 变更哈希 16→8 hex）。真正的大杠杆是 tools 那 80%。
- 168 个工具里 `browser` 前缀独占 25 个、`team` 14、`session`/`desktop`/`task` 各 9、`heartbeat`/`memory` 各 6 —— **绝大多数是多智能体 / 会话管理 / 浏览器 / 桌面自动化 / 媒体生成，日常单聊完全用不到**。

## 2. 目标 / 非目标

**目标**
- 单轮上下文 50.6K → **~20K（削 ~60%）**，**零能力损失**：全部 168 工具仍经目录可见、经 `tool_search` 可达。
- 决策权 100% 留模型；harness **零消息级过滤**。
- 收益随 MCP/hub/skill 工具增多而**放大**（它们天然进目录、按需加载）。

**非目标**
- 不裁剪 system prompt（身份 / memory / `<available_skills>` 本次一律不动）。
- 不做"按消息意图动态筛工具"（R10 明禁）。
- 不引入 feature flag（生产功能始终编译；逃生舱用 `core=["*"]`）。
- 不碰 `src/harness/`（R10 文件预算零增长）。

## 3. 宪法依据（为什么不违红线）

- **R10 第 2 不（本次修订）**: 由"不做工具过滤"精确化为"不按**消息意图**做工具过滤 / 相关性评分"，并加渐进披露例外注。依据：`src/tools/scoped/mod.rs:159-174` **早已**有 `is_allowed`(allowlist) / `is_permission_denied`(权限) / `is_healthy`(健康) 三道**静态** `retain`——工具呈现层做静态分区从来合规，本设计是同层同性质的新成员。
- **R7 主权**: 加载与否、加载哪个，全由模型调 `tool_search` 发起。harness 对每条消息做的事完全一样（发 core + 目录 + tool_search），不看内容。
- **R8 全可达**: 全量工具 `name + 一行 desc` 始终在目录里可见；任何一个都能被 search 出来执行。
- **R9 智慧在 prompt**: 披露规则（"你有 core 工具集；需要别的先 tool_search"）写进 prompt 模板，零额外 LLM 调用。
- **落点**: `src/tools/`（呈现层）+ `src/context/`（prompt 层）+ 一个内建 `tool_search` 工具。**不进 `src/harness/`**。

## 4. 架构

**一句话**: registry 仍持全量 168 工具不变；只在**组装 provider `tools` 数组的边界**做静态三分——

```
                         ┌─ core 工具 (静态 config 声明)  ──► 完整 schema 进 `tools` 数组
全量 registry (168) ─────┼─ 已 load 工具 (从 transcript 归约) ──► 完整 schema 进 `tools` 数组
                         └─ 其余工具                     ──► 仅 name+desc 进 system 的「工具目录」block
                                                          + 始终注入 `tool_search` 元工具
```

- 分区在 `src/tools/scoped/`（或 provider adapter 组 tools 处），与现有三道 `retain` 同层。
- `allowed_tools`/`denied_tools` **先生效**（决定"这个 agent 有哪些工具"）→ 再做 core/deferred 三分（决定"哪些常驻 vs 进目录"）。正交分层。

## 5. 组件（4 个，单一职责）

### 5.1 core 声明 — `[tools] core = [...]`（config）
- 静态工具名列表，集中、可见、可被 R8 工具（`self_config`）修改。
- `core = ["*"]` = 全常驻 = 旧行为（逃生舱 / 向后兼容）。
- 默认值 = §7 的草案清单。

### 5.2 工具目录渲染器 (catalog renderer)
- 把非 core 工具渲成一个 system-prompt block：每行 `工具名 — 一行描述`（~15-25 tok/行）。
- **直接复刻 `src/skill/prompt.rs` 的 `SkillPromptBudget`**（`max_entries` / `max_chars` / 稳定排序 / full-vs-compact 降级），不重新发明预算逻辑。
- 放 system prompt 尾部（稳定前缀，对官方 Anthropic 用户可缓存；对 kimi 用户无差别）。

### 5.3 `tool_search` 元工具（内建，始终注入）
- 输入：`select:name1,name2`（精确取）或关键词查询（模糊搜）。镜像本 harness 的 ToolSearch。
- 输出：匹配工具的完整 JSON schema，注入后即可调用。
- 从全量 registry 解析，不受 core 分区影响（但仍受 `allowed_tools`/权限门约束）。

### 5.4 loaded-set 归约（从 transcript）
- 已被 `tool_search` 成功加载的工具名 = **对本会话 transcript 的纯归约**（扫历史里成功的 `tool_search` 结果）——无新持久态、可重建（合 A3/F12 纯 reducer 方向）。
- 下一轮 `tools` 数组 = `core ∪ loaded`。粘性：本会话内一旦 load 不必再 search。

## 6. 数据流（两回合走一遍）

**回合 A —「你好」**: 装配 → 发 core (~18 工具全 schema) + 目录 (168 行) + `tool_search`。模型直接回。**零 round-trip**，请求体 ~20K。

**回合 B —「帮我用浏览器查 X」**: 模型从目录知有 `browser_*`（但无 schema）→ 调 `tool_search("select:browser_navigate,browser_snapshot,...")` → 拿 schema → 下一轮正确调用。**多 1 次 round-trip（"core 税"）**；之后 browser 工具粘性常驻本会话。

## 7. core 工具草案 (~18)（⚠️ 草案，待删改）

**遴选原则**: core = 单聊高频 + 基础文件/代码/网络/记忆/规划。凡**浏览器 / 桌面 / 多智能体(team/session/agent/node/acp/a2a/arena) / 任务管理(task) / 媒体生成 / hub / 心跳 / 定时 / 生命周期**一律 defer。

| tool | 归类 | 为何 core |
|------|------|-----------|
| `ask_user` | HITL | 阻塞式澄清，模型随时要用 |
| `bash` | 执行 | 基础 shell |
| `code_exec` | 执行 | 跑代码/脚本 |
| `code_check` | 执行 | 快速校验 |
| `file_read` | 文件 | 日常刚需 |
| `file_write` | 文件 | 日常刚需 |
| `file_edit` | 文件 | 日常刚需 |
| `file_ops` | 文件 | 列/移/删 |
| `search` | 网络 | 通用搜索 |
| `web_fetch` | 网络 | 抓 URL→markdown |
| `memory_search` | 记忆 | 召回 |
| `remember` | 记忆 | 落记忆 |
| `skill_read` | 技能 | 按需读 skill 正文（配合 `<available_skills>`）|
| `skill_list` | 技能 | 列可用 skill |
| `scratchpad` | 规划 | 多步草稿计划 |
| `note_manage` | 笔记 | 记忆三支柱之 note |
| `system` | 系统 | `open_path` 等（打开生成的文件）|
| `tool_search` | 元 | 始终注入（不计入 168）|

**⚠️ 边界项（你定是否纳入 core）**: `recall_context` / `recall_events`（与 memory_search 部分重叠）、`document_extract`（读文档）、`apply_patch`（与 file_edit 重叠）、`goal` / `loop`（长跑单元入口）。

**defer 大赢面**: `browser`(25) `team`(14) `session`(9) `desktop`(9) `task`(9) `heartbeat`(6) `memory_*`浏览类(5) `hub`(5) `agent`(5) `node`(4) + 媒体/acp/arena/lifecycle/workflow/plan 等 ≈ **~150 工具进目录**。

## 8. 配置面 / 向后兼容 / 分层

- **新增 config**: `[tools] core: Vec<String>`（默认 = §7 清单）; catalog 预算复用 skill 那套常量。
- **向后兼容**: `core = ["*"]` → 旧全量行为。无 feature flag。
- **分层顺序**: `allowed_tools`/`denied_tools` → core/deferred 三分 → 权限 Deny → 健康门（后两道沿用现有 `retain`）。
- **每 agent 可覆盖**: `AgentDefinition` 可选带自己的 core 覆盖（power agent 可 `core=["*"]`）。

## 9. 错误处理与鲁棒性

- **模型盲调未加载工具**（知名字、无 schema、瞎猜参数）: 返回结构化错误「此工具需先 `tool_search` 加载」，**不自动执行**（盲猜参数几乎必错，correctness 优先）。镜像本 harness 的 `InputValidationError` 提示。
- **目录预算溢出**: 复用 skill budget 的 compact 降级；若仍超，`log` 明示丢弃了哪些（"no silent caps"）。
- **模型侧风险（诚实）**: 机制依赖模型理解渐进披露。sonnet-4-5（当前主 agent 模型）原生会用（本 harness 同款）；换弱模型可能 search 不干净 → 写入 Risks，靠 core 集覆盖高频来兜底。

## 10. 测试与 token 门

- **单元**: 三分逻辑（core/loaded/deferred）; 目录渲染 + 预算降级; `tool_search` 名→schema 解析（尊重 allowlist/权限）; loaded-set 从 transcript 归约; 盲调报错。
- **集成**: 回合 A 的 `tools` 仅含 core; `tool_search` 后回合 B 的 `tools` 含已加载工具; `core=["*"]` 退回全量。
- **Token 门（硬指标）**: 抓一次真实「你好」请求体，验证 ~50.6K → ~20K（±，取决于 core 最终成分）。这是成败判据。

## 11. 风险

| 风险 | 缓解 |
|------|------|
| 非 core 工具首次用多 1 次 round-trip | core 集覆盖日常高频；round-trip 只在切到重型能力时发生 |
| 弱模型 search 不干净 | 主 agent 是 sonnet-4-5；prompt 明确引导；盲调有纠正提示 |
| 官方 Anthropic 用户：tools 数组随 load 变化 → 破 tools 块缓存断点 | core+目录在 system 稳定前缀仍可缓存；kimi 用户本就无缓存无影响 |
| core 清单选错致高频触税 | 清单是 config，可被 R8 工具热改；先按 §7 草案，观测后调 |

## 12. 已锁决策 / 遗留

**已锁**（brainstorming 对齐）:
- 触发机制 = 渐进式工具披露（单 agent，非静态多档）。
- core 大小 = 均衡 ~18-20。
- loaded-set = 从 transcript 归约（非显式缓存）。
- core 放 config 列表（非工具 manifest 标记）。
- 保留全量工具目录（名+一行）+ 粘性加载。

**遗留给实施计划确认**:
- core 草案清单的边界项最终取舍（§7 ⚠️）。
- catalog block 在 system prompt 的确切插入位置与预算常量取值。
- `tool_search` 的查询语法细节（是否支持关键词模糊搜，还是仅 `select:` 精确取）。
