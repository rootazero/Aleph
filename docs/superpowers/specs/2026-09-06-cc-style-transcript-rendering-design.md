# Claude-Code 风格转录渲染 — 设计规格 (CC-Style Transcript Rendering — Design Spec)

**Date:** 2026-09-06
**Branch:** `worktree-cc-render-r1`（worktree，`.claude/worktrees/cc-render-r1`；完成后停在分支上，不合 main）
**Scope:** `shared/protocol` · `shared/ui_logic` · `src/builtin_tools/file_ops` · `src/gateway/handlers` · `interfaces/tui` · `interfaces/webchat`
**Status:** 六节设计逐节获用户认可（2026-09-06），待 spec 文件审阅
**Reference material:** pi 母体 `T:\Github\pi`（packages/tui · packages/coding-agent）、`pi-cc-extensions`、`pi-claude-code-tui`。`pi-tasks` / `pi-subagents` 本机不存在（真实包 `npm:@tintinweb/pi-{tasks,subagents}`），其**展示契约**由 pi-cc-extensions 自带的 Agent/Task 渲染器与 pi 示例 `subagent` / `todo` 扩展恢复。

> English summary: Port the *ideas* of Claude-Code-style transcript rendering (one-line tool rows
> with a folded `⎿` output slot, rich diffs, markdown enhancements incl. mermaid, click/keyboard
> expand, back-to-bottom, a measured context breakdown, CC-style chrome) into Aleph. The design
> pulls all presentation logic into `shared-ui-logic::transcript` as pure data→data functions,
> adds a typed UI side-channel (`ToolResult.presentation: FileChange`) and two RPCs on the wire,
> and leaves Panel (Leptos) and TUI (ratatui) as thin painters. Three phases: A wire + shared core,
> B TUI, C Panel.

---

## 1. 背景与扫描结论

六份扫描（pi 母体 1940 行、插件目录 3791 行、Panel 543 行、TUI 468 行、wire 契约 623 行、shared-ui-logic 地图）得出的**已验证**现状：

| 维度 | 参考做法 | Aleph 现状 | 缺口 |
|---|---|---|---|
| 工具调用摘要行 | 按工具名的摘要表 + 首选键回退 + 名称人性化；spinner 帧 = f(now) | Panel `tool_headline` 只显参数值、`ToolKind::from_name` 启发式误判（`_exec`→Bash、`_search`→Search）；TUI 3–5 行带框盒子、`k=v` 截 120 字、**全部渲染在正文之上不按时间交错** | 摘要器共享；TUI 改单行 |
| 结果折叠 | 母体全局 Ctrl+O，各工具折叠上限不同；cc-tui 3 **物理行**；pi-cc-ext 折叠零正文 | Panel `MAX_INLINE_LINES=8` **逻辑行**、不持久化；TUI **工具输出根本不渲染**，`/tools verbose` 与 `All` 字节相同 | 两参考矛盾 → 已裁定（§2） |
| 分组 | 连续同工具合并、edit/write 不分组、单子项解散 | Panel `ExploreGroup` 已有；TUI 无 | 下沉共享 |
| Rich diff | `diff` 库行+词级、4 行上下文、执行前预览；pi-cc-ext 写前捕获、12%/26% 混色、预算、5 种 unavailable | **服务端零 diff**；Panel 用 `similar` 从 `old_string/new_string` 重算：**多段 `file_edit` 空 diff `+0 -0`**、`file_ops` 写入无 diff、`file_write` 无旧侧；TUI 无 | 服务端结构化（§2） |
| Markdown | 表格/LaTeX；pi-cc-ext：mermaid、`> [!TYPE]`、裸 URL、**无 file:line** | Panel pulldown-cmark 3 flag + syntect，无 mermaid/提示块/autolink；TUI 手写 837 行子集无表格无高亮 | 共享变换链 + 两端解析器统一 |
| 鼠标/命中 | APC 零宽标记 → 区域表；母体布局树有 rect 但命中**未实现** | Panel DOM 点击已有；TUI **鼠标捕获从未开启** | ratatui `Rect` 直接命中 |
| 回底 | 停靠部件、等真实帧、未知=在底 | Panel 已完善（`chat_scroll`）；TUI 无 | TUI 补齐 |
| Context 分项 | `/context` chars/4 **估算** + provider 对账 + Other 行；`percent:null` 一等状态 | 服务端 `LayerSize` **实测**已有但只有 CLI；Panel 环 + tooltip，inspector 已删；TUI 无分项无成本 | 新 RPC，Aleph 给实测 |
| 状态栏/页眉 | Clawd、`❯`、`model │ Context 23% │ $0.04`、动词 | TUI 全框输入盒、无页眉、无成本、主题 66 行硬编码 5 角色同色 | 版式与手感（§2） |
| 回放 | 恢复的工具无 start 事件会永远转圈 | Panel **刷新丢全部工具卡**；`trace.by_runs` 已开放未用；wire 工具结果 = 给模型的 8000-token 截断副本，无全量 RPC | 接线 + 新 RPC |
| 契约 | — | 同帧家族三份定义；协议 `RunSummary` 缺 `context_tokens/window`；`ReasoningBlock` 零生产者；25/43 Panel API 模块手写孪生 | 本轮触及帧沉协议 |
| 共享层 | — | `shared-ui-logic` 9 模块仅 2 真共享；`leptos` feature 零引用；3 个死 pub；TUI `auto_scroll: bool` 是 `chat_scroll` 削弱孪生 | 落点 + 熵减 |

---

## 2. 用户裁定（2026-09-06）

| # | 裁定 | 选择 |
|---|---|---|
| R-1 | 范围与顺序 | 三阶段 **A（wire + 共享内核）→ B（TUI）→ C（Panel）**，同一 worktree 分支 |
| R-2 | 折叠策略 | **cc-tui 式露 2 行**：折叠态 `⎿` 槽显示前 2 个**物理行** + 1 行 `… +N lines`；按工具覆写：`Read` 不露正文、`Bash` 取尾部、`Edit/Write` 露 `+a -d` + 前 2 个 hunk 行；展开全量、超长走可滚动区域 |
| R-3 | diff 生成端 | **服务端结构化** `FileChange{hunks}`，挂 `ToolResult.presentation`，不进模型文本；alephcore 新增 `similar`；词级 LCS 留客户端 |
| R-4 | Mermaid | **Panel 真渲染**（惰性加载随 `dist/` 打包的 mermaid.js，`securityLevel:"strict"`，沙箱 iframe，完成后才渲）；**TUI 框住源码**，不做 ASCII |
| R-5 | TUI 身份 | **取版式与手感不取身份**：`ℵ` 页眉、金色 `❯`、含成本状态行、模式提示行、按 locale 的 30–40 个精选动词、语义 token 主题（dark/light/terminal）+ `/theme` |

---

## 3. 架构

```
                 aleph-protocol（唯一形状源）
   FileChange · Hunk · HunkLine · Unavailable · Presentation
   ContextBreakdown · LayerSize · RunSummary(+context_tokens/window) · ModelInfo(+context_window)
                            │
 服务端  src/builtin_tools/file_ops/diff.rs   →  compute_file_change()（唯一 similar 调用点）
         五个变更工具挂 presentation           →  在拼模型文本之前剥离
         src/gateway/handlers                  →  trace.tool_output · context.breakdown
         session_events                        →  presentation 随 tool_call_completed 持久化
                            │  wire
 shared-ui-logic::transcript（纯函数，无 leptos / ratatui / 网络）
   view_model · summarize · fold · group · diff_view · md_enhance · context · turn_summary · affordance · theme_tokens
                            │            │
              Panel (Leptos)           TUI (ratatui)
              只做 DOM/CSS             只做 Line/Span + Rect 命中表
```

**否决的替代**：新建 `aleph-render` crate（`shared-ui-logic` 已被两端依赖且有 `markdown_stream` 先例，第二个真源）；逻辑塞进 `aleph-protocol`（协议 crate 只持 wire 形状，CLI 等客户端不该背算法）。

**这个设计让什么变难**：`shared-ui-logic` 成为 Panel 与 TUI 的公共变更点，单侧快速试错略慢（刻意：判据 §16 孪生主动同步）；`FileChange` 让大改动的 `ToolEnd` 帧变大（hunk 有上限，超出置 `TooLarge` 并留 `trace.tool_output` 拉全量）。

---

## 4. Phase A · wire 与服务端

### 4.1 协议类型（`shared/protocol/src/`）

```rust
// file_change.rs
pub struct FileChange {
    pub path: String,
    pub kind: FileChangeKind,            // Created | Modified | Deleted
    pub hunks: Vec<Hunk>,                // 4 行上下文；总行数超 MAX_HUNK_LINES 时清空并置 unavailable
    pub added: u32,
    pub removed: u32,
    pub unavailable: Option<Unavailable>,
}
pub struct Hunk { pub old_start: u32, pub new_start: u32, pub lines: Vec<HunkLine> }
pub struct HunkLine { pub tag: LineTag /* Ctx | Add | Del */, pub text: String }
pub enum Unavailable { Binary, TooLarge, PreImageUnavailable, Encoding, ToolFailed }  // 闭集，渲染原因，绝不回退成 Created

// tool_result 侧信道（单段 edit = 长度 1 的 Vec；apply_patch 多文件 = 多元素）
pub enum Presentation { FileChanges(Vec<FileChange>) }
// ToolResult 增 `presentation: Option<Presentation>`（serde default，老客户端忽略）

// context.rs
pub struct ContextBreakdown {
    pub layers: Vec<LayerSize>,                    // name + bytes，来源上一次真实请求的 prompt pipeline
    pub tools: Vec<ToolSchemaSize>,                // name + schema_bytes（工具注册表序列化实测）
    pub messages_tokens: Option<u64>,
    pub provider_reported: Option<UsageTokens>,    // 压缩后无新 usage → None
    pub context_window: Option<u32>,
    pub turn: u64,
}
```

`RunSummary` 协议侧补 `context_tokens` / `context_window`，网关**从协议类型构造**（方向修正）；`ModelInfo` 补 `context_window`。所有新字段进 `frame_census.rs` 视野。

### 4.2 服务端

1. `src/builtin_tools/file_ops/diff.rs`（新）：`compute_file_change(path, before: Option<&str>, after: Option<&str>) -> FileChange`；`similar::TextDiff::from_lines` + 4 行上下文分组；二进制/非 UTF-8 → `Binary`/`Encoding`；行数上限 → `TooLarge`（保留 added/removed）。
2. 挂载点：`file_edit`（单段 + 多段 `edits: Vec<EditOp>`，合成**一个** `FileChange`）、`file_write`（**写前先读**旧文件；读失败 → `PreImageUnavailable`；文件不存在 → `Created`）、`apply_patch`（逐文件一个 `FileChange`）、`file_ops` 的 write/edit 臂。工具失败 → `ToolFailed`。
3. `presentation` 在 `src/tools/scoped/dispatch.rs` 拼模型文本**之前**剥离；随 `tool_call_completed` 写入 `session_events`。
4. RPC `trace.tool_output {session_id, run_id, tool_call_id, offset, limit}` → `{text, total_bytes, truncated}`，读磁盘原件，单页 ≤ 2 MB；缺失 → `Err(NotFound)`。
5. RPC `context.breakdown {session_id}` → `ContextBreakdown`；只读测量，不改 system prompt 字节。
6. plan 实时路径：两端统一 `aleph_protocol::plan::snapshot_from_tool_output`，替换所有 `result.get("snapshot")`。

### 4.3 守卫（每条答得出"什么时候变红"）

- census：从内置工具注册表**派生**"会改文件的工具"清单——用工具 trait 上已有的文件变更谓词；若没有这样的谓词，就在 trait 上加一个 `mutates_files()` 并由它派生，**不写名单**（判据 §3/§5）。断言每个都挂 `presentation`；新增变更工具未挂即红。
- 模型文本排除：把哨兵字符串塞进 hunk，断言拼出的模型文本搜不到。
- 协议 round-trip：用协议类型构造 → serde → 解析，两端同一份类型（判据 §10）。
- `trace.tool_output` 缺失 → 必须 `Err`。
- `aleph-server prompt-size` 前后字节相等（presentation 没漏进 prompt）。

### 4.4a 档案核对后的修正（2026-09-06，写 Plan A 前对照源码提取的接口档案）

spec 初稿的四个前提与源码不符，按源码修正，Plan A 以此为准：

1. **`file_ops` 没有 write/edit 臂**（只有 List/Move/Copy/Delete/Mkdir/Search/BatchMove/Organize/Stats）。挂载点是 `file_edit`、`file_write`、`apply_patch` 三个。另有两个内容写入者 `skill_manage`（写 SKILL.md）与 `node_file`（集群节点文件推送）**本轮不纳入**——前者是配置面、后者的字节不进对话；trait 谓词 `mutates_file_content()` 对它们保持 `false`，记入待办。
2. **侧信道走既有的"提升"先例，不改工具 trait 返回类型**：工具在自己的 JSON 输出里放 `_presentation` 键；`apply_layer_two` 在把值扁平化成模型文本**之前**把它提升到 `ToolOutputMetadata.presentation`（与 `images` 同一条路）。持久化在 `SessionEvent::ToolResult`（事件日志本来就整体序列化 `ToolOutput`），**不是** trace 行；`trace.by_runs` 回放时按 `call_id` 从事件日志读回补进 `AgentTraceToolCallEnd.presentation`。harness 只把 `on_tool_call_done` 的 `result` 参数从 `Option<&Value>` 换成 `Option<&ToolOutput>`（每个调用点本就持有 `ToolOutput`，行数不变，R10 棘轮零增量）；`FlowStreamEvent::ToolCallDone` 增 `presentation` 字段；drain 处写到 wire `ToolResult.presentation`。
3. **`file_write` 实际从不读旧文件**（`is_byte_equal_existing` 长度不等即短路）——写前捕获是新增逻辑，在同一把路径锁下读。
4. **`LayerSize` 不在会话中保留，且生产路径给 prompt 管线的工具表是空的**（schema 走原生 tool_use）。因此：① 在 `cache.rs` 的稳定/动态分段构建上加"一次渲染同时测量"的变体，替换现有 `ALEPH_PROMPT_SIZE_TRACE` 的二次渲染；② 新增 `thinker::PromptSizeRegistry`（`CapabilitySlot`，每会话保留最新一份）；③ 工具 schema 字节在 orchestrator 组装本轮工具表处量；④ `provider_reported` 服务端留 `None`，客户端用已收到的实时 `ContextGauge` 填后再 `reconcile`。
5. 协议 `ToolResult.metadata: Option<Value>` 零读零写 → **CUT**（`skip_serializing_if`，wire 字节不变）；网关侧 `ToolResult` 与协议侧字节相同 → 改为 `pub use aleph_protocol::ToolResult`（消一对孪生）。`RunSummary` 两份**没有任何转换函数**，wire 直接发网关结构、协议侧解析时静默丢字段——本轮只补齐两个字段并加键集对账测试，整体统一另议。

### 4.4 Phase A 熵减

CUT `plugins/diff-viewer`（Extism，零引用）；协议 `RunSummary` 削弱版消失。`ReasoningBlock` 零生产者**不动**（超范围，记入 FEATURE_LOCATOR 附录 D 待裁）。

---

## 5. Phase A · 共享呈现内核 `shared-ui-logic::transcript`

| 模块 | 契约 |
|---|---|
| `view_model` | `TranscriptEntry = UserText \| AssistantText \| Reasoning \| ToolRow \| ToolGroup{rows} \| TurnSummary \| SystemNotice`；`ToolRow{id, tool, summary, status: Pending\|Running{since}\|Ok{dur}\|Err{dur,msg}, body: None\|Text\|FileChanges(Vec<FileChange>), expanded}`；**按时间交错**；`settle_resumed()`：无 start 有结果 → 落定，无 start 无结果 → Pending 不转圈 |
| `summarize` | `summarize(tool, &args) -> {display_name, args_text}`；显示名表 `file_read→Read · file_edit→Edit · file_write→Write · shell→Bash · grep→Grep · find→Find · web_fetch→Fetch · web_search→Search · apply_patch→Patch`；MCP `server__tool → Server · Tool`；其余 snake/camel → Title Case；参数：Read `path[:l1-l2]`、Bash 首行截 80、Grep `pattern in path`、Patch `N files`、subagent `name: task…40`；回退键序 `path→file_path→command→query→pattern→url→name→id→action→message` |
| `fold` | `fold(lines, width, policy) -> {visible_rows, hidden_rows, hidden_lines}`；`unicode-width` 换行后数**物理行**；逻辑行预扫描保险（最多看 `rows×4` 行）；`FoldPolicy::for_tool`：默认 2 行 + 提示，Read 不露正文，Bash 尾部，Edit/Write `+a -d` + 2 hunk 行；宽度未知 → 按 80 列 |
| `group` | 连续只读类（Read/Grep/Find/Fetch/`file_ops` read/search）合并；Edit/Write/Bash 永不；单子项解散；容忍 ≤3 段空文本 |
| `diff_view` | `rows(&FileChange, width, expanded) -> Vec<DiffRow{old_no, new_no, tag, spans}>`；相邻 Del/Add 词级 LCS（预算折叠 200k 格、展开 1M 格，超预算退回行级）；`mix(rgb, rgb, t)`；**只做 unified** |
| `md_enhance` | 文本→文本预处理：`> [!NOTE\|TIP\|IMPORTANT\|WARNING\|CAUTION]` → `Admonition`；裸 URL 自动链接（平衡定界符裁边，跳过 code）；`path:line[:col]`（需已知扩展名、不在代码内）；` ```mermaid ` → `Block::Mermaid(src)`；流式期间只在围栏闭合后做重变换（复用 `safe_freeze_offset`）；P8：正则只配 markdown 结构 |
| `context` | `reconcile(&ContextBreakdown) -> Vec<Row>`：System / Persona / Skills / Tools(N) / Memory / Messages / **Other**；provider 总数偏差 >0.1% 以 provider 为准；`None` → `?` |
| `turn_summary` | `Ran 3 commands, read 2 files, edited 1 file · 42s`，≥2 工具才生成，存数据渲染时格式化 |
| `affordance` | `hint(Modality::Mouse \| Key(name), hidden)`；`spinner_frame(now_ms)`（braille 80 ms，纯 f(now)）；`verb(locale, seed)` 中/英各 30–40；`worked_for(dur)` |
| `theme_tokens` | `SemanticColor` 枚举（约 30 角色：`tool_pending/ok/err · diff_add/del/ctx · admonition_* · prompt · dim · accent…`）；Panel 映射 CSS 变量名、TUI 映射 `Color`；**角色表只此一份** |

**守卫**：① alephcore 侧测试从工具注册表派生名单，断言每个工具有显式摘要条目**或**在显式"故意回退"名单里；② `fold` 用 6 KB 压缩 JSON 单行断言物理行数≠逻辑行数；③ URL 裁边与 `path:line` proptest；④ `reconcile` 的 Other = 总量 − 各项且 ≥ 0；⑤ 词级 LCS 超预算退回行级（定时断言）。

**同笔熵减**：CUT `leptos` feature、`DefaultConnector`、`FailureStage::RpcTimeout`（确认不可达后）、`PromptInjectionCheck.reasons`（若无消费者）。

---

## 6. Phase B · TUI（`interfaces/tui/`）

- **转录模型**：`ChatMessage` 三变体 → 消费共享 `TranscriptEntry`；工具行按时间交错；`finish_tool_execution` 保留结果文本（每行 ≤ 64 KB，更长展开时经 `trace.tool_output` 拉）。
- **工具行**（替换 `tool_block.rs`）：

```
⏺ Read(src/gateway/mod.rs:1-120)
  ⎿ Read 120 lines
⏺ Bash(cargo test -p aleph-tui)  ✗ 1.2s
  ⎿ error[E0433]: failed to resolve …
    … +41 lines (ctrl+o to expand)
⏺ Edit(src/tui/theme.rs)  +12 -3
  ⎿  30 │ - pub const DEFAULT_THEME
     30 │ + pub fn resolve(role: SemanticColor)
    … +2 hunks (ctrl+o to expand)
● Explored 4 files · 0.8s
```

- **展开**：`Ctrl+O` 全局翻转 + 鼠标单击提示行展开该行、双击（400 ms）折叠；`/tools verbose` = 默认展开的真实语义。
- **鼠标**：crossterm 鼠标捕获；滚轮；绘制时构建 `RegionTable{rect, kind, row_id}`（优先级 show-more > back-to-bottom > 折叠提示 > 展开卡）。
- **滚动**：`auto_scroll: bool` → 共享 `state::chat_scroll`；`[ ↓ Back to bottom · Ctrl+End ]` 停靠；`Scrollbar`。
- **Markdown**：`markdown.rs` 换 pulldown-cmark → `Span`（与 Panel 同 flag 常量）；表格移植 pi 4.5 列宽算法；有序/嵌套列表、删除线、任务列表、hr、引用、提示块左轨、链接 URL 暗色；`syntect` 懒加载（后台线程加载语法集，未就绪先纯文本；结果进 `LineCache` 只高亮一次）。
- **外观**：页眉 `ℵ Aleph 26.x · <model> · ~/cwd (branch)` 作首条转录条目；输入去框：平面分隔线 + 金色 `❯` + 空时暗色 `Try "…"`，多行自增长；状态行 `● model │ ctx 23% (50k/200k) │ $0.042 │ tier·mode·think │ ⚡2 │ ⠹ Pondering… 12s`，宽度不足按优先级丢段；提示行 `⏵⏵ auto · ctrl+o expand · ctrl+end bottom`，打字时压缩为模式标签；成本 = `session.usage` + `RunSummary.estimated_cost_usd` 累加；动词每轮随机、7 s 重掷；收尾 `✻ Worked for 12s`。
- **主题**：`SemanticColor → Color`；预设 `dark` / `light` / `terminal`（`Color::Reset` + ANSI-16）；truecolor 检测（`COLORTERM`）；`/theme <name>` 持久化。
- **新命令**：`/context` 覆盖层（`reconcile` 行 + 占比条，`?` 表示未知）。
- **熵减**：删 `tool_block.rs`、旧 `markdown.rs`、`btw_panel.rs` 第二个 spinner、`AgentPanelData` 三个陈旧 `#[allow(dead_code)]`。
- **刻意不做**：悬停高亮、键盘逐行光标、并排 diff、ASCII mermaid、inline（非全屏）模式。

---

## 7. Phase C · Panel（`interfaces/webchat/`）

- **`ToolCard` 消费共享 `ToolRow`**：标题 `Read src/x.rs`（显示名 + 参数）；状态字形同 `SemanticColor`；折叠视觉 CSS `line-clamp: 2`，隐藏数由共享 `fold` 按容器实测列宽算，提示 `affordance::hint(Mouse)`。
- **展开持久化**：`expanded_events` 按 `session_id` 存 localStorage（LRU ≤ 500 id）；"全部展开/折叠"按钮 + `Ctrl+O`。
- **分组**：`ExploreGroup` → 共享 `group`。
- **Diff**：吃 `presentation.FileChanges`（每个文件一块）；hunk 头 + 双行号 gutter + `color-mix()` 12%/26% + 共享词级 `<mark>`；`unavailable` 原因芯片；`TooLarge` → 覆盖层经 `trace.tool_output` 拉全量。
- **Markdown**：先 `md_enhance` 再 pulldown-cmark（flag 常量与 TUI 同一份）；提示块 `<aside class="admonition-*">`；自动链接沿用 `sanitize_link_url`；`path:line` → 打开 Panel 已有文件预览（不可直接复用则先做复制路径并在报告标明）；Mermaid：`<iframe sandbox="allow-scripts" srcdoc>` + `dist/vendor/mermaid.min.js` + `securityLevel:"strict"` + `postMessage` 高度；完成后才渲；失败退回代码框 + 错误首行。
- **Context 分项面板**：`InspectorTarget::ContextBreakdown`，点击百分比环打开；行来自 `reconcile`；数字直接可见。
- **刷新重建**：`chat.history` 之后调 `trace.by_runs`（分页）重建工具行 / `FileChange` / usage / `terminate_reason`，再 `settle_resumed()`。
- **每轮摘要行**（≥2 工具）。
- **熵减**：删 `ToolKind::from_name`、`tool_headline`、`ExploreGroup`、`edit_body` old/new 路径、未挂载的 `CardHeader/CardTitle/CardDescription/CardContent/SuccessMessage/ErrorMessage`；`chat/state/mod.rs` **只抽出**工具行状态到 `state/tool_rows.rs`。
- **刻意不做**：reasoning 改逐消息、并排 diff、phone 专属版式调整。

---

## 8. 失败语义（fail-closed）

| 场景 | 行为 |
|---|---|
| diff 算不出 | `unavailable` 五选一渲染原因；**永不**退成 `Created` |
| `trace.tool_output` 缺失 | `Err(NotFound)` → "输出不可用"，不显空白 |
| 压缩后无新 usage | `provider_reported = None` → `?`，不显 0 |
| 回放行无 start 事件 | `settle_resumed()` 落定，不转圈 |
| 宽度未知 | 按 80 列折叠 + 逻辑行保险 |
| 终端无鼠标 / 捕获失败 | 键盘路径不变；提示文案切 `ctrl+o` 形态 |
| syntect 未加载完 | 纯文本，不阻塞帧 |
| mermaid 失败 / 无沙箱 | 代码框 + 错误首行 |
| localStorage 不可用 | 展开态仅进程内 |
| MCP / 未知工具 | 显式回退摘要，出现在 census 的"故意回退"名单 |

---

## 9. 验证矩阵

| 阶段 | 命令 |
|---|---|
| A | 六条最小可信集 + `cargo test -p aleph-protocol` + `cargo test -p shared-ui-logic` + 新 census / round-trip / NotFound 测试 |
| B | 上 + `cargo test -p aleph-tui -p aleph-cli` + 本机 Windows Terminal 真机（鼠标 / truecolor / `/context` / `/theme`） |
| C | 上 + `cargo test -p aleph-panel --lib`（先警告再错误）+ `just wasm` + Node 夹具驱动 chrome-devtools 断言五个 DOM 事实（多段 edit 有 hunk · `file_write` 新建显 Created · 长 shell 折叠 2 行 + 正确隐藏数 · mermaid 出 iframe · 刷新后工具卡仍在） |
| 全 | `cargo clippy --workspace --all-targets` 零警告（棘轮）；`aleph-server prompt-size` 前后字节相等 |

主机注意（见 memory）：alephcore `--lib` 构建约 13 min 超过 Bash 10 min 上限，需 detach；`--test '*'` 需 `-j 1`；共享 `CARGO_TARGET_DIR` 只用于 check / `--lib` / clippy；`--lib` 基线 20 红在 `49b0475fb`，**比名字不比数量**。

---

## 10. 文档（完成后，独立 commit）

- 新增 Tier-2 `docs/reference/TRANSCRIPT_RENDERING.md`：视图模型契约、折叠策略表、摘要表、diff 模型、`md_enhance` 规则、theme token 表、mermaid 沙箱边界、两个新 RPC。
- FEATURE_LOCATOR：§5.13 TUI 新轮次、§6.1 Panel 新轮次、工具 presentation 侧信道条目；附录 D/E 只收**新形状**（预期两条：「给模型的副本 ≠ 给 UI 的副本，同一 wire 字段不能兼任」「折叠以物理行计，逻辑行只做保险」）；附录 A 体检更新。
- CLAUDE.md：子系统路由表**加一行** `shared/ui_logic/` → TRANSCRIPT_RENDERING.md；其余不动。

---

## 11. 提交与执行

- 英文 `<scope>: <description>` 小步提交，每个带 `Claude-Session:` 尾注；预计 A ≈ 6、B ≈ 6、C ≈ 6、文档 2。
- spec 批准后 → `writing-plans` 出任务清单 → `subagent-driven-development`：实现 / 测试 / 批量改动派给 Opus（跨 crate 契约、diff、TUI 重做）或 Sonnet（清理、文档、机械替换）；主会话只做拆解、派发、验收与真机验证。
- 完成后**停在分支不合 main**，合并由用户裁定。

## 12. 回滚

每阶段独立可回滚：A 的协议字段全部 `serde(default)`，老客户端忽略；B/C 各自只改一个 interface crate。任一阶段回滚不需要回滚前一阶段。
