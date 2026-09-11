# TRANSCRIPT_RENDERING.md — 转录呈现：文件变更侧信道与共享渲染核

> **Tier 2**。落点索引在 [FEATURE_LOCATOR.md](FEATURE_LOCATOR.md) **§6.13**；判据触发器在它的**附录 E.0 / E.1 / E.3 / E.4 / E.7**。
> spec 母本：[`docs/superpowers/specs/2026-09-06-cc-style-transcript-rendering-design.md`](../superpowers/specs/2026-09-06-cc-style-transcript-rendering-design.md) ·
> 计划：[`docs/superpowers/plans/2026-09-06-cc-render-phase-a-wire-and-shared-core.md`](../superpowers/plans/2026-09-06-cc-render-phase-a-wire-and-shared-core.md) ·
> 逐 Task 台账（裁定 / 更正 / 残余，约 420 行）：`.superpowers/sdd/2026-09-06-cc-render-phase-a-wire-and-shared-core/progress.md`。
> 脱敏信任模型全文在 [SECURITY.md](SECURITY.md)；工具输出 ingress / 预算级联在 [FEATURE_LOCATOR §3.14](FEATURE_LOCATOR.md)；prompt 预算与截断在 [§1.2](FEATURE_LOCATOR.md)。
>
> ⚠️ **本文不复制代码拥有的事实**（判据 §1）。每个常量、每个枚举成员、每条顺序都带着**它的所有者**（符号 + 文件）出现；
> 读到与代码不一致时**代码是权威**。行号是**对当前 HEAD 的断言**，最会腐烂——引用前重读。
>
> 🛑 **本文描述 Phase A 的交付**：**服务端线路 + 共享渲染核**。A→B→C 的顺序是用户自己裁定的，不是遗漏。
> **更新（2026-09-11）**：Phase B（TUI）已完成，`shared/ui_logic/src/transcript/` 不再是零调用者——
> 见 §5.1 的已偿注记。Phase C（Panel）仍**没有建**。
> 本文仍只描述 Phase A 那半边；B 侧的现状在 `docs/superpowers/plans/2026-09-11-cc-render-phase-b-tui.md`。

---

## 0. 这一层在解决什么 / What this layer is for

一次 `file_edit` 在今天的转录里只留下一句 `Replaced 1 occurrence in src/a.rs`。人看不到改了什么；
要看只能自己去开文件，而那时文件已经是改后的样子。Claude Code 式转录的核心不是配色，是
**工具结果除了模型要读的那段文本之外，还带一份给人看的结构化呈现**——而这两份必须是**同一次调用的两个面**，
不是两次各自计算的结果。

Phase A 交付三样东西，各自独立可验证：

| 交付 | 是什么 | 落点 |
|---|---|---|
| **呈现侧信道 Presentation side-channel** | 文件变更工具把一份结构化 `FileChange` 挂在结果上，**模型看不到**，两条腿把它送到人面前 | `shared/protocol/src/file_change.rs` · `src/builtin_tools/file_ops/diff.rs` · `src/exec/masker.rs` |
| **两个读面 RPC** | `trace.tool_output`（一次工具调用的**未截断**输出，分页）· `context.breakdown`（这个会话**上一次真正发出去的** prompt 由什么占着） | `src/gateway/handlers/{tool_output,context_breakdown}.rs` · `src/thinker/prompt_size_registry.rs` |
| **共享渲染核 shared core** | 折叠 / 摘要 / 分组 / diff 行 / markdown 前处理 / 上下文行——**data → data**，两端各自绘制 | `shared/ui_logic/src/transcript/`（11 个模块，60 条 `#[test]`） |

**熵减的一半也在这一轮里**：`ToolResult` 的网关孪生（`src/gateway/event_emitter/types.rs`）曾是一个手写副本，
带一个 `metadata: Option<Value>` 字段——**零写者**，`skip_serializing_if` 让它一次都没上过线。
它被**删除**而不是搬进 `aleph_protocol`（协议 crate 不收投机字段），网关侧现在是 `pub use aleph_protocol::ToolResult;`，
`StreamEvent::ToolEnd` 与线上帧因此不可能再漂。

---

## 1. 呈现侧信道 (Presentation Side-Channel)

### 1.1 类型与所有者

全部住在 `shared/protocol/src/file_change.rs`，由 `aleph_protocol` 顶层再导出：

| 类型 | 语义 | 备注 |
|---|---|---|
| `Presentation` | 工具结果能携带的 UI 侧信道。`#[serde(tag = "kind")]`，今天只有 `FileChanges { changes: Vec<FileChange> }` | **闭集**：第二种（比如表格）是一次刻意的添加，不是一个 `_` 臂 |
| `FileChange` | `{ path, kind, hunks, added, removed, unavailable }` | 一次调用改了几个文件就有几条；单次编辑是长度 1 的 `Vec` |
| `FileChangeKind` | `Created` / `Modified` / `Deleted` | 由 `(before, after)` 的 `Option` 组合派生，见 §1.2 |
| `Hunk` | `{ old_start, new_start, lines }`，两个起点都是**1-based** | |
| `HunkLine` | `{ tag, text }` | ⚠️ `text` **逐字携带文件内容**——这是脱敏必须走到它的原因，见 §2 |
| `LineTag` | `Ctx` / `Add` / `Del` | 线上是 snake_case（`ctx` / `add` / `del`），由 `wire_key_names_are_locked` 钉住 |
| `Unavailable` | **为什么没有 hunk**。见 §1.3 | |
| `CONTEXT_LINES = 4` | 每侧保留的上下文行数 | 与 pi `edit-diff.ts` 同 |
| `MAX_HUNK_LINES = 400` | 线上携带的 hunk 行总数上限，超出降级为 `TooLarge` | 也是 §2 第二遍脱敏扫描的代价上界 |
| `PRESENTATION_KEY = "_presentation"` | 工具放进自己 JSON 的键名 | 派发层 hoist 之后**移除**它 |

`ContextBreakdown` / `ToolOutputPage` 那一组在 `shared/protocol/src/context_breakdown.rs`，见 §3。

### 1.2 谁生产它

**唯一的 `similar` 调用点**（alephcore 范围内）是 `src/builtin_tools/file_ops/diff.rs`：

- `compute_file_change(path, before: Option<&str>, after: Option<&str>) -> FileChange` —— `before == None` ⇒ `Created`，
  `after == None` ⇒ `Deleted`，两者皆有 ⇒ `Modified`，**两者皆无是编程错误**并产出 `Unavailable::ToolFailed`。
- `presentation_for(changes) -> Presentation` —— 包一层。
- `MAX_DIFF_INPUT_BYTES = 2 MiB` —— 超过就不 diff。

三个工具通过 `AlephTool::mutates_file_content() -> true` 声明自己会改文件内容，并各自挂上 `_presentation`：
`file_edit`（`file_ops/edit.rs`）· `file_write`（`file_ops/write.rs`，为此**新读一次 pre-image**）· `apply_patch`（`file_ops/apply_patch.rs`）。
`plan_delete` 的 pre-image 读取是 **best-effort**：读不到就发 `PreImageUnavailable`，**不会**把一次删除变成失败
（这是 fail-closed 的方向：少一个 diff，不是少一次删除）。

⚠️ `AlephToolDyn::mutates_file_content` 的默认实现是 `false`，这**只**对手写的 dyn 实现（MCP adapter / 测试替身）生效——
`T: AlephTool` 的 blanket impl（`src/tools/traits.rs`）转发真实答案。若那条转发缺失，每个 builtin 都会答 `false`，
而 §5.4 的普查会变成一个**永远不可能变红**的谓词（判据 §2）。

### 1.3 `Unavailable` 是一个闭集：渲染器**说出理由，从不猜**

枚举自己的 doc 就是这条契约的所有者。今天五个成员，**每一个都有生产者**：

| 成员 | 什么时候 | 生产者 | `added` / `removed` |
|---|---|---|---|
| `Binary` | 任一侧不是合法 UTF-8，或嗅探窗口里有 NUL | `diff::read_pre_image` | 0 |
| `TooLarge` | hunk 行数超 `MAX_HUNK_LINES` / 输入超 `MAX_DIFF_INPUT_BYTES` / diff 跑超 `DIFF_TIME_BUDGET` | `diff.rs` 三处 + `read_pre_image` | ⚠️ 见下 |
| `PreImageUnavailable` | 真正的 I/O 错误（含 `metadata` 失败但不是 NotFound——**不知道它存不存在，就不许说它是新建的**） | `diff::read_pre_image` | 0 |
| `ToolFailed` | 工具自己失败，什么都没写 | `apply_patch` / `compute_file_change` 的 `(None, None)` | 0 |
| `Redacted` | 一个密钥模式**跨了不止一行 hunk**，整份 diff 被扣下 | `exec::masker::mask_file_change` | **仍然精确** |

⚠️ **`Encoding` 已被删（2026-09-07）**。它的语义是「文本解出来了但按行切分失败（例如孤立代理项）」——
那是 UTF-16 的问题：Rust 的 `String` 天生合法 UTF-8，`str::lines()` 不会失败，所以这个工作区里
**任何生产者都产不出它**。一个闭集里不可达的成员会教渲染器去处理一个到不了的情形（判据 §2）。
真要回来，必须和它的生产者在同一笔改动里回来。
同批修的是另一半：`file_write` 与 `apply_patch(delete)` 此前各用一个 bool / 一个 `Option` 把
**三种原因**（超字节上限 / 二进制 / I/O 错误）压成一个 `PreImageUnavailable`，所以 `Binary`
的生产者数目实际上是 0——现在两条路共用 `diff::read_pre_image`，原因即线上词。

**为什么不复用一个现成成员**（`Redacted` 的存在理由，也是这个枚举扩展机制的示范）：
`Redacted` 那一刻**没有失败、没有过大、内容解码正常**——拿其中任何一个当标签都是**一个错的标签**，
而闭集契约里错的标签比缺的贵（判据 §17）。新增一个成员会让 `shared/ui_logic` 的穷尽 match 编译不过，
**那是机制在工作，不是障碍**。

**⚠️ `TooLarge` 下的 `0 / 0` 永远读作「没算」，绝不读作「没变化」**（`diff.rs` 的 `MAX_DIFF_INPUT_BYTES` doc 是这条不变量的所有者）：
`Modified` 那一侧的行数各自都不是真实统计量，所以被硬置 0；而 hunk 行数的那道上限**只会在一个真有变化的 diff 上跳闸**，
所以真实统计量下 `TooLarge + 0/0` 不可达。`transcript::diff_view::stats_label` 就是靠这条不变量在那种情况下渲染
「diff 不可用」而不是 `+0 -0`。第三个生产者（`DIFF_TIME_BUDGET`）遵守同一条：**一个零变化的 diff 花不掉 250 ms**。

**⚠️ 有界的 Myers（2026-09-07）**：`compute_file_change` 现在走
`TextDiff::configure().deadline(..)`，`DIFF_TIME_BUDGET = 250ms`。字节上限只挡「比 2 MiB 大」的输入，
2 MiB 以内两份差异极大的 ~40k 行文本仍是 O(N·D)，而它**同步跑在 tokio worker 上**、
`MAX_HUNK_LINES` 又是**算完之后**才裁——一份注定被丢弃的 diff 会被先足额付一遍。
选 250 ms 的理由：能活过 `MAX_HUNK_LINES = 400` 的 diff 本身就是个小编辑脚本，
即便在 2 MiB 输入上也只要个位数毫秒；而 `apply_patch` 是**每个文件调一次**，上限得给这个乘数留余量。
⚠️ **超时之后 `similar` 不报错**：它停止寻找 middle snake，改吐一份**合法但非最小**的脚本
（整段删 + 整段插），`TextDiff` 上**没有任何「我被截断了」的标志位**。所以代码在**构造之后立刻问时钟**：
过了 deadline 就一律降级为 `TooLarge` + `0/0`，**不报一个自己没算出来的数**。
误判窗口是「刚好在 deadline 前几微秒算完」，落在 fail-closed 一侧。

### 1.4 它怎么到达客户端，以及为什么模型看不到

```
工具返回 JSON: { …, "_presentation": {…} }
      │
      │  src/tools/scoped/dispatch.rs::apply_layer_two
      ▼  result_processing::hoist_presentation(&mut out.value)  ← #[must_use]，取走并 REMOVE 那个键
ToolOutput.metadata.presentation : Option<Presentation>          （src/session/events.rs）
      │
      ├──► session_events（SessionEvent::ToolResult.output）——落盘，重放腿的源头
      │
      └──► HarnessCallback::on_tool_call_done 的带外 `presentation` 参数
             （orchestrator/harness_bridge/callback.rs：`result.and_then(|o| o.metadata.presentation.clone())`）
                   │
                   └──► StreamEvent::ToolEnd → ToolResult.presentation → 线上 `stream.tool_end`
```

**hoist 必须发生在扁平化之前**：`out.value` 一旦 `to_string()`，`_presentation` 就变成模型要读的那一坨 JSON 里的
几千字节噪声（R9）。`hoist_presentation` 因此带 `#[must_use]`——它**修改**入参并返回**唯一一份**被取走的东西，
丢掉返回值就是静默丢掉一个工具的 UI diff 而编译器一言不发（它的孪生 `hoist_inline_images` 一直有这个属性）。

**presentation 骑在 `output` 旁边，从不回折进 `output`**（`event_drain.rs` 的注释与
`tool_end_carries_the_presentation_beside_the_plain_text_output` 一起钉住）。回折会让线上出现两份 diff
并毁掉那句给人读的纯文本 `output`。

---

## 2. 两条腿，一处派生 (Two Legs, One Derivation)

同一份 diff 会经**两条完全不同的路**到达**同一个人**：

| 腿 | 面 | 代码 | 源 |
|---|---|---|---|
| **实时** | `stream.tool_end` 帧 | `gateway::event_emitter::RedactingEmitter`（`redacting.rs`） | 内存里的回调 |
| **重放** | `trace.by_runs` 的 `tool_call_completed` | `gateway::handlers::trace_replay::{handle_by_runs, presentations_for_session}` | `session_events` 事件日志 |

两条腿**共用同一个走查**：`crate::exec::masker::mask_presentation`。这不是洁癖——
**一份打了码的拷贝加一份没打码的拷贝不叫脱敏**：同一个人从两条路看到同一次调用，其中一条泄露就等于泄露。

`mask_presentation` / `mask_file_change` 逐层**不带 `..` 地解构**，变体 match **没有通配符**：
`Presentation` / `FileChange` / `Hunk` / `HunkLine` 上任何一个新的**带文本字段**都是编译错误。
这条轴是调用方的臂级穷尽性够不到的——`presentation` 正是作为**一个已存在变体内部嵌套类型上的新字段**到达的
（`redacting.rs` 自己的注释说它的穷尽 match 保证「新变体是编译错误」，而这次来的是**新字段**）。

### 2.1 为什么跨行的密钥要扣下**整份** diff

`src/exec/secret_patterns.rs` 里**只有一个多行模式**：PEM 私钥
（`-----BEGIN[A-Z ]*PRIVATE KEY-----[\s\S]*?-----END…`），它要求 BEGIN 与 END 在**同一个字符串**里。
一个 `.pem` 写下来是每行一段 base64，**逐行扫描永远匹配不到**；
而 `file_write` 的 `output` 只有一句 `Wrote N bytes to <path>`，所以那份密钥的正文**只存在于 hunk 里**。

处置是 **DEGRADE，不是 join-mask-split**：

1. 先逐行 mask（单行模式在这里被处理掉，行数与 hunk 边界完好）；
2. 把**已经打过码的**那些行 join 起来再 mask 一次；
3. 第二遍有变化 ⇒ 说明匹上了一个多行模式 ⇒ `hunks.clear()` + `unavailable = Some(Redacted)`。

**不能把 join 后的结果切回去**：PEM 的替换文本**本身含换行**，切回去会改变行数，
于是 `old_start` / `new_start` / `added` / `removed` 全部开始撒谎。
**统计量保持精确是因为它们本来就还是真的**——「我有计数但不能给你看 diff」是渲染器说得出口的一句话，
一份被悄悄改坏的 diff 不是。

两个边界，都是刻意的：

- **join 的范围是整个 `FileChange`，不是每个 `Hunk`**——一把中段作为未改动上下文存活下来的密钥会被切进两个 hunk，
  两个被显示的半截都在泄露；按 hunk join 恰好漏掉**编辑**这个场景。跨 hunk 的误报代价是一份被扣下的 diff，那是 fail-closed 的一侧。
- **`hunks.is_empty()` 提前返回**，所以一个已有的理由不会被一个假的 `Redacted` 覆盖掉。
  这是**完整**而不是部分的：全仓产地已枚举——`file_ops/diff.rs` 在设 `TooLarge` 的同一口气里 `hunks.clear()`，
  其余一律经 `FileChange::unavailable(..)`（构造出来就没有 hunk），所以「有理由且有 hunk」这个状态不可达。
- 第二遍扫描的成本上界是 `MAX_HUNK_LINES = 400`，与第一遍同阶，且 hunk 为空时整段跳过。

⚠️ `FileChange.path` **也被 mask**，而这不是本轮的判断——它沿用孪生腿早就付过账的那条裁定
（`execution_engine/unattended_redacting_sink.rs` 的模块 doc）：**没有白名单，不对「哪个字段算自由文本」做判断**，
identifier 形状的字段只花一次不会命中的正则，比一条需要人对每个新字段重新分类的规则便宜。
而且「一个路径永远不像凭据」**不是我们能替运维承诺的**——`[[security.mask_patterns]]` 允许装任意正则。

### 2.2 缝：脱敏住在哪一侧，为什么两侧不一样

`trace.by_runs` 的两个来源**在相反的两侧**脱敏，而这是**自洽的**（`trace_replay.rs::presentations_for_session` 的 doc 是它的所有者）：

| 来源 | 在哪脱敏 | 为什么 |
|---|---|---|
| `task_traces` 行（handler 吐出的其余一切） | **写时**，`execution_engine::unattended_redacting_sink::mask_trace_event` | 那些行还要喂 channel 推送和 WS `agent_trace` 镜像——在**产地**打一次码覆盖所有消费者 |
| `session_events`（**只有** presentation 这一张 map） | **服务时**，在 handler 里 | 事件日志是**模型的上下文**，写时打码会毁掉模型读回来的东西（12-factor A2） |

**两个方向都明令禁止**：给 `session_events` 写时打码（毁模型上下文）· 给 `task_traces` 服务时再打一遍码
（冗余，且掩盖了是哪个产地在担保）。任一动作都是拿两个派生换掉一个正确的派生。

### 2.3 ⚠️ attended / unattended 的不对称——已知开口

写时那条脱敏**只包 unattended run**：`run_loop/inner.rs` 的 `if unattended` 同时包住 trace sink 与 **event emitter**。
所以纠正过的不变量是：

> **unattended run 在实时帧与写盘两处都脱敏；attended run 两处都不脱敏。**

推论两条，都要写下来：

1. `trace.by_runs` 在这个计划碰它之前，就已经在为 attended run 服务未脱敏的工具文本。**既存的、归属在别处的，不是本轮的**。
2. §1 那个缺陷（同一帧上 `output` 打了码而 `presentation` 没有）**只在 unattended run 上存在过**，
   所以它的修复对它的作用域是完整的。

而重放腿**无条件**脱敏，因为**重放拿不到 attended/unattended 信号**——没有任何东西把它记在行上或会话上，
「按写时的规则来」在这里不可实现，fail-closed 是唯一可用也是正确的选择：泄露不可逆，打码可逆。

**被接受的后果，并在调用点连同成因一起注释**：对一个 attended run，重放出来的 presentation 是打过码的，
而**同一条**重放事件上的 `result.output` 没有，实时帧上两者都是明文。这是 §1 那个缺陷符号翻转过来的样子。
实际代价比这句话听起来小——masker 只匹凭据形状的字符串，所以 attended 用户看到的是**自己 diff 里恰好坐着一个凭据的那一处**被打码，
不是一份被打码的 diff。

**⚠️ 别用「把这处 masking 删掉」来消解这个不一致。** 正解是把 attended/unattended 标记记到 trace 行（或会话）上，
让重放能对齐写时——那是 schema + 写路径的改动，本轮明令禁止。
**重访触发条件**：任何人因为别的理由要往持久化的 run 状态里加 attended/unattended 信号时，顺手把这条关掉；
以及任何人在 Phase B/C 里觉得这个不对称碍事时，来这里而不是去删那行 masking。

---

## 3. 两个新 RPC

两个都是**寻址面**：调用方点名一个 session，那个 key 经 `visibility::session_visible` 做 KeyChecked，
拒绝复用 `visibility::not_found_response`，所以一个别人的 key 与一个不存在的 key **逐字节同形**。
两个都进了 `method_census.rs`（`Class::Open`）与 `method_visibility.rs::SCOPED_METHODS`（`Treatment::KeyChecked`）。

### 3.1 `trace.tool_output` —— 一次工具调用的未截断输出，分页

`src/gateway/handlers/tool_output.rs`。

线上 `tool_end` 帧和 `messages` 投影携带的都是**模型面**的那份拷贝：`apply_result_budget` 在溢出时
把值换成预算内文本、把原文卸载到 blob。所以「把整段输出给我看」诚实的答案有**三种形状**，
而这个方法**说出**是哪一种（`aleph_protocol::ToolOutputSource`），不假装它们一样：

| `source` | 含义 | `truncated` |
|---|---|---|
| `Inline` | 结果本来就在预算内，事件日志里那段就是全部 | 只由分页决定 |
| `Persisted` | 溢出过，沿 `[Full output persisted: <path> …]` marker 读到了 blob | 只由分页决定 |
| `Expired` | 溢出过，但 blob 被扫走（7 天 TTL）或**被拒**——预算内文本是仅存的东西 | **恒 true，且治不好** |

`truncated` 是**一个 bit 覆盖两件事**（协议类型自己的 doc 定义了这个析取）：后面还有页 **OR** 剩下的永久没了；
`source` 是分辨它们的那一半。

**被拒有三条路，一条都不许读成「这里有些字节」**（判据 §8）：文件不在了 · 在 store root 之外 · **是为另一次调用写的**。
第三条是本轮新加的绑定，理由是**混淆代理**：marker 住在工具文本里，而工具文本是**工具打印的任何东西**——
一次读到攻击者所写文件的 `file_read`、一次 echo、一个抓来的网页，都能含一行 `[Full output persisted: `，
而 `extract_persisted_path` **按设计扫描每一行**（Layer-2 可能在真 marker 上方插一段错误摘要）。
所有 session 的 blob 共用一个 root，所以只有 containment 的话，session A 输出里一行伪造 marker 就能把服务端的读引到 session B 的 blob 上。

其余机制：`MAX_TOOL_OUTPUT_PAGE_BYTES = 2 MiB` 是**天花板**；**地板**（`limit: 0`，客户端发得出且上游不拦）住在 `page()` 里，
因为**声称自己会推进的是它**——`page` 的 `end` 向上取整到一个完整字符边界，并返回**真实的起点**而不是被请求的那个，
于是协议自己写下的 `offset + text.len() < total_bytes ⇒ truncated` 才真的成立。
**脱敏在分页之前**：一个跨页边界的凭据在逐页 masker 眼里是两个匹不上的半截，而页的两端正是逐页 masker 的盲区。

⚠️ **`SessionEvent::ToolError` 不被服务**：一次失败的工具调用发的是 `ToolError { error: String }` 而不是 `ToolResult`，
所以拿一个失败调用的 id 来问会得到 `RESOURCE_NOT_FOUND` 而不是错误正文。这是**刻意划在范围外**的——
`ToolOutputSource` 没有对应的臂，硬造一个会让 `source` 撒谎。**触发条件**：哪天某个渲染器真的想分页一次失败，
那就是要做的**协议**改动，而不是把 handler 悄悄放宽。

### 3.2 `context.breakdown` —— 上一次真正发出去的 prompt 由什么占着

`src/gateway/handlers/context_breakdown.rs`，背后是 `src/thinker/prompt_size_registry.rs`。

**为什么是登记簿而不是重新渲染一遍**：「这个会话的 prompt 里有什么」只有一个诚实的答案——
**在那些字节被产出的那一刻记下来的那个**。事后重新派生等于拿此后已经移动过的输入（一个轮后焊上去的 strategy、
一个新装的 skill、一条刚写的记忆）重跑一遍管线，然后把结果当成「发出去的那份」报出来。
那正是 `prompt_build.rs` 里那个被删掉的 per-session prompt LRU 的缺陷形状。

**一轮一次写，整条记录替换**：`PromptSizeRegistry::record_turn(session_key, Option<PromptLayout>, tools)` 是**唯一写者**。
两个写者加一条合并规则，会产出一条**带着上一轮的 layers 和这一轮的 tools、却标着旧轮号**的记录——
一份描述了一个从未存在过的 prompt 的记录。替换**取消了交错本身**，所以没有规则可以搞错。
`layout` 是 `Option` 因为「这一轮压根没建系统 prompt」是关于这一轮的**事实**，不是把它写成空表加零的理由。
`MAX_TRACKED_SESSIONS = 256`，逐出用登记簿自己锁下的单调 `write_seq` 而不是毫秒时钟（同毫秒内 256 次插入会让
`min_by_key` 在随机的 HashMap 序上挑一个）。

**两个字段刻意留空而不是填上**：

- `provider_reported` —— 这个方法**永远**发 `None`。它测的是 **prompt**，而 provider 的计数只有 provider 的响应带得来，
  拿本地算的总数冒充是一句自信的谎（同 Task 8 对 `reconcile` 的裁定）。
  ⚠️ **客户端前置条件**：`shared_ui_logic::transcript::reconcile` 在这个字段为 `None` 时返回 `total: None` / `percent: None`，
  所以**照原样渲染服务端响应会得到没有总数、没有百分比条的一排行**。客户端必须先从它已经在收的实时 `ContextGauge` 把这个字段填上，
  再调 `reconcile`。这句话同时写在**协议类型**、**`reconcile` 自己的 doc** 和 handler 的 doc 上，因为那些才是客户端作者会读的东西（判据 §7）。
- `messages_tokens` —— 会话历史那一半有它自己、派生方式不同的估算器（`harness_bridge::context_estimate`）。
  从这扇门报出去会把一个问题变成两个答案。

**⚠️ layer 行是 trim 之前的组装**：`layers` 把字节归给发出它们的那一层，而这种归属**只在
`prompt_budget::fit_dynamic_suffix_with_content` 头尾裁剪 dynamic 尾巴之前存在**——裁完之后没有哪一层拥有被裁掉的字节。
于是对一个超预算的会话，行**高估**了模型收到的东西，而那恰恰是这个功能存在的那一个场景。
`dynamic_bytes_sent` 因此上了线，**它而不是那些行才是 dynamic 半边的权威尺寸**：

- 比 dynamic 行**小**：trim 咬到了；
- 比 dynamic 行**大**：`prompt_builder/cache.rs` 在层被测量**之后**焊进了一个后管线的 `<strategy>` 块，没有哪一层拥有那些字节。
  （今天 Cached 路径上不可达——只有 `subagent_spawner` 调 `with_strategy` 而那是 Basic 路径、不记录——
  但那是**关于现有调用方的陈述，不是关于这个函数的性质**，而那处焊接离测量点五行。判据 §5。）
- stable 前缀是 trim 从不碰的**受保护地板**，所以 `Σ(stable 行) + dynamic_bytes_sent` 就是**真正发出去**的那个 prompt 的大小。
- `None` 只意味着「这条记录根本没有 prompt 测量」（那一轮没建系统 prompt），**从不**意味着「什么都没裁」。

**记录点是调用方，不是 `build_system_prompt`**：那个函数有**两个**调用方，只有一个是真回合——
`runner_impl.rs` 的那个是回合，`estimate_context` 那个建的是一份**刻意弱化**的 prompt
（`TurnEnvelope::none()`、无摘要、无 manifest、无工作区、空 query）用来给可缓存开销定价，**而且它传的是真的 session id**。
在那里记录会让估算路径覆盖掉真实记录并推进 `turn`，把一份弱化的拷贝**当作那个 prompt** 发布出去，
而且那条路一点都不冷门——它喂着上下文仪表。所以 `build_system_prompt` **返回**尺寸、什么都不记，
由**知道自己要的是哪一种构建**的调用方去记。

**没有记录的东西**：只有主循环的 prompt。子代理的 prompt（`Basic` 组装路径，`subagent_spawner`）被测量、被 trace，
但**从不写进登记簿**，所以一个子代理的 session key 会**永久**答 `RESOURCE_NOT_FOUND`，而不是「还没测，待会再问」。
这是诚实的，但它**不是暂时的**——接上它需要在 spawner 那边加一个写者。

**R9 中性性是被测试的**：这个功能要求测量与组装是同一次遍历，于是本轮把 `prompt_pipeline.rs` 的**五条**遍历
（`execute` / `execute_with_mode` / `execute_stable_with_mode` / `execute_dynamic_with_mode` / `layer_breakdown`）
塌成一条私有的带测量遍历，五个公开签名全部不变。
`the_collapsed_traversal_matches_a_direct_append` 保留一份 `#[cfg(test)]` 的**塌陷前**参考实现，
在真实 `default_layers()` 上跨 `AssemblyPath` × `PromptMode` **全叉积**断言字节相等；
路径与模式列表**由类型经穷尽 match 派生**，所以加一个变体会让测试编译不过而不是悄悄收窄扫描面（判据 §5）。

---

## 4. 共享渲染核 `shared/ui_logic/src/transcript/`

**data → data，没有渲染、没有 signal、没有终端。** 两端各自绘制这些函数**决定**的东西，
所以它们不可能对一次折叠、一份摘要、一个 hunk 或一个颜色角色产生分歧。
**必须在 `default-features = false` 下编译**（TUI 构建）——这里任何东西都不许 feature 门控、不许伸手去够 leptos / web-sys。

| 模块 | 回答什么 | 关键导出 |
|---|---|---|
| `theme_tokens` | 两端共用的**语义颜色角色**唯一名册（Panel 映到 CSS 变量名，TUI 映到 `ratatui::Color`） | `SemanticColor` · `ALL_SEMANTIC_COLORS` · `mix_rgb` |
| `fold` | 把工具结果正文折成几**物理**行——**先 wrap 再数**（一行 minified JSON 就是六十行终端；数逻辑行会全给你看） | `fold` · `wrap_physical` · `FoldPolicy` · `DEFAULT_COLLAPSED_ROWS` |
| `summarize` | 工具在行上**叫什么**（Aleph snake_case → Claude Code 词表；其余 humanize；MCP 是 `Server · Tool`），以及哪个参数值一行 | `DISPLAY_NAMES` · `display_name` · `summarize` · `CallSummary` |
| `group` | 把连续的只读工具行并成一条 `Explored N calls` | `group_entries` · `MIN_GROUP` · `MAX_GAP_TEXT` |
| `view_model` | 转录作为数据：条目按时间顺序，一条工具行**自成一个条目**（Claude Code 把工具与文本交错） | `TranscriptEntry` · `ToolRow` · `ToolGroup` · `RowStatus` · `READ_ONLY_DISPLAY_NAMES` |
| `diff_view` | 从线上 `FileChange` 到可绘制的行；成对 Del/Add 上做第二次 token LCS 标出改动的词，带预算 | `diff_rows` · `stats_label` · `word_spans` · `COLLAPSED_DIFF_ROWS` |
| `md_enhance` | markdown 前处理（admonition / 裸 URL / `name.ext:NN` / mermaid 围栏），**不引正则 crate** | `enhance` · `find_path_refs` · `linkify_bare_urls` · `Block` |
| `affordance` | 两端逐字共用的小呈现决定（时长、spinner、动词、locale） | `fmt_duration_ms` · `spinner_frame` · `verb` · `Locale` |
| `context` | 把 `ContextBreakdown` 变成 `/context` 视图的行，含 provider 对账与显式 `Other` 余量 | `reconcile` · `ContextRow` · `PROVIDER_TOLERANCE` |
| `turn_summary` | `Ran 3 commands, read 2 files, edited 1 file · 42s`——存成**数据**，绘制时再成文 | `summarize_turn` · `MIN_TOOLS_FOR_SUMMARY` |

三条在别处会被「统一」掉、所以写在这里的裁定：

- **`settle_resumed` 从不伪造 `Ok`**：任何 `Running` 行恢复后落到 `Pending`。少报，绝不多报。
- **`summarize_turn` 的时长与分类计数都只累计 `is_terminal()` 的行**，全部非终态时整条返回 `None`——
  一个全零的条目渲染成「Ran 0 commands」是在断言什么都没发生，而真相是**不知道**。
  `ToolGroup::headline` **刻意**不跟它对齐（它在活着的行旁边现绘，"N calls" 复述的是读者自己数得出来的东西），这是 Phase B/C 第一次绘制它时要定的措辞。
- **`reconcile` 在 provider 总数**小于**各部分之和且超出 `PROVIDER_TOLERANCE` 时，`total` 与 `percent` 都报 `None`**，
  保留逐项行、不发 `Other` 行。两个测量互相矛盾时「这个上下文有多满」诚实的答案就是不知道。
  **不对称是刻意的**：各部分**低于**总数时保留 `Other` 行。
- **`READ_ONLY_DISPLAY_NAMES` 里没有 `"Files"`（`file_ops`）**：那个工具把只读与破坏性动作复用在一个名字上
  （`src/security/dangerous_tools.rs`），一次删除会被渲染成被动的探索。丢掉这一条是 fail-closed 的方向
  （一次只读的 `file_ops` 只是没被折进分组）；把 `is_read_only` 改成认动作则要把原始参数穿过 view model 送进两端。

---

## 5. 本轮**没有**做的事 / 已知开口

> 一份只描述顺利路径的参考文档，正是这一轮整天在花钱避免的那个东西。以下每一条都是知情的开口，不是遗漏。

### 5.1 🛑 `shared/ui_logic/src/transcript/` 没有渲染器——**一笔有日期的债，不是沉默**

> **已偿（2026-09-11，Phase B B1–B8，全阶段完成）**：`aleph-tui` 现在是这棵树的客户端——`theme_tokens` /
> `view_model` / `fold` / `affordance` / `summarize` / `md_enhance` / `turn_summary` / `context`
> 都有生产调用点。`context.breakdown` 的第一个客户端是 `/context` 覆盖层
> （`interfaces/tui/src/tui/{app/context_view.rs,widgets/context_overlay.rs}`）。
> **`trace.tool_output` 仍然零客户端**——下面这份测量对它依然成立。Panel（Phase C）未动。
> 保留原文是因为它是一次带谓词和 commit 的测量，不是一句会过期的断言（判据 §18）。

**测量（2026-09-07，在本分支 HEAD 上）**：`transcript::` 在 `interfaces/` 里**一次都没有出现**；
`aleph-tui` 与 `aleph-panel` 都依赖这个 crate，但用的是它的**别的**模块（`state::agent_panel`、`markdown_stream`、
`connection`、`state::SessionKnobs`、`state::chat_scroll`），从来不是这一个。
`src/` 那侧唯一的引用是 `src/tools/presentation_census.rs` 里的一条 `#[cfg(test)]` 依赖
（它调真的 `display_name`，见 §5.4）。整棵树 11 个模块 / 2 272 行 / **60 条 `#[test]`**，**零调用者**。
⚠️ 那个 58 是 `grep -c "#\[test\]" shared/ui_logic/src/transcript/*.rs` 数出来的，谓词是「这棵树里的测试函数」；
台账里那个 **144 是 `shared-ui-logic` 整个 crate `--lib` 的通过数**，谓词不同——两个数都对，别互相替代（附录 C.1）。

⚠️ **台账（`progress.md` 判据 §17 那条）只点名了 `diff_view.rs`；实测的范围更宽——是整个 `transcript/` 树。**
`diff_view` 之所以被单独点出来，是因为它是 `Unavailable` 闭集契约**唯一的**渲染器：
`stats_label` 里那几个 `Some(Unavailable::…)` 臂就是「说出理由，从不猜」这句话在代码里的全部落实，
而它今天没有人调用。（**别在这里维护成员个数**——上一版写着「六个」，`Encoding` 一删就静默变假了；
数目的所有者是枚举自己。）
⚠️ **同一句话在共享核里被复现过一次，本轮修掉了**：`diff_rows` 只遍历 `change.hunks`，
于是一个 `Redacted` / `TooLarge` 的 change 与一次真正的空编辑返回**逐字节相同**的值，
理由只活在 `stats_label` 这个**另一个函数**里——正是 `Redacted` 那条裁定要防的形状
（「一个空 hunk 列表读起来像『这次改动什么都没碰』」）被搬到了上一层。
现在 `diff_rows` 返回 `DiffView::{Rows, Unavailable}` 这个**和类型**：拿不到 rows 就必须先匹配另一条臂，
忘记问理由的渲染器**编译不过**，而不是「显示空白且什么都不说」。修法刻意不是加一条注释——
**两个函数之间的约定**正是这次失效的东西。

**这是被批准的，不是违规**：A→B→C 的顺序是用户自己裁定的（Phase A = 服务端 + 共享核，B = TUI，C = Panel），
共享核比它的渲染器早落一期就是计划在正常运行。但判据 §17 要求一份「展示用」的东西必须指得出渲染它的那一行，
指不出就是 CUT 而不是「以后再接」。**这里诚实的形态是一笔带关闭条件的、有日期的债**：

> **关闭条件**：Phase B（TUI）或 Phase C（Panel）任一落地，即为本条关闭。
> **若 B 与 C 都不发生**，`transcript/` 就是一棵带着完整测试套件、没有消费者的死代码树，届时**删除**它比重新推导便宜——
> 但前提是有人被告知它在这里。

### 5.2 ⚠️ blob 归属绑定是**名字前缀**，不是集合成员

`ToolResultStore::blob_belongs_to_call` 判的是文件名是否以 `{sanitize(tool_call_id)}_` **开头**
（那个函数自己的 doc 是这件事的所有者，含完整的可达性分析）。**前缀不必停在名字边界上**：
一个本身是别的 id 的**结构性前缀**的 `tool_call_id` 会吃下它下面的一切——用 `toolu` 去问会匹上
`toolu_01ABC…_bash.txt`，也就是 store 里每一次 Anthropic 形状的调用。

- **经仓内任何 id 生成器都不可达**：三个生成器全是定长形状加一个新鲜 uuid nonce
  （`providers/delta.rs` · `providers/ollama.rs` · `providers/protocols/gemini/sse.rs`），攻击者**缩短不了自己的 id**。
- **上游逐字透传 id 时可达**：`providers/protocols/openai_chat/proto_impl.rs` 原样带过 `tc.id`，
  Gemini 的 `sse.rs` 在载荷点名时也一样，而 Aleph 支持任意 `base_url`。
  一个敌意的「OpenAI 兼容」端点因此可以自己发 `id: "toolu"`。`tools.invoke` 不接受调用方给的 id。
- **正解是精确匹配**（`stem == format!("{}_{}", sanitize(call), sanitize(tool))`，`tool` 从同一次调用的
  `SessionEvent::ToolCallRequested.name` 读），**它是被阻塞的，不是被忘了**：
  `browser_tools::offload_full_content` **刻意**把 blob 归在**调用方**工具而不是快照工具名下，
  而 `browser_tools/exec.rs` 有一条断言逐字钉住这一点。不先处理那个例外就上精确匹配，
  等于拿一个窄泄露换来**一整类**浏览器结果的假 `Expired`。
- **交给用户的开放决定**（2026-09-07 裁定：本轮只修注释不修谓词）。仍需衡量的是那个取舍，而衡量需要测量。
  ⚠️ 本轮之前**根本没有任何绑定**，所以发出去的东西严格优于原状。

### 5.3 ⚠️ canonicalize 关不掉硬链接——一个**断言开口仍然开着**的测试

`ToolResultStore::read_call_blob` 一次 canonicalize、两道闸（containment + ownership）。
**符号链接是关掉的**：解析出来的名字是受害者的，ownership 因此拒绝。
**硬链接关不掉**：硬链接**没有目标可解析**，两个名字是同一个 inode 的对等目录项，
`canonicalize()` 原样返回你递给它的那个名字——于是 ownership 闸读到的是**攻击者挑的名字**，而字节是受害者的。
**任何基于名字的检查都修不了它。**

这个残余被 `a_hardlink_is_not_closed_by_canonicalization` 钉住——**一条断言这个开口开着的测试**，
所以哪天真把它关上了，测试会**按名字变红**并强制在同一笔改动里改掉那份 doc。这是「一个已知的洞被记在一句会腐烂的注释里」的反面。
边界也被诚实地圈了出来：造出那条链接需要 store root 内的写权限，而那已经意味着能直接读受害者的文件。所以是残余，不是升级。

⚠️ 它的孪生 `a_symlink_is_judged_by_what_it_resolves_to` 在**这台主机上未被证明**：
`New-Item -ItemType SymbolicLink` 要管理员权限（Developer Mode 关着），所以那条测试尝试建链接、
失败时打印 `SKIPPED … The branch is unproven here.` 然后返回。它在 Linux/macOS 和任何带
`SeCreateSymbolicLinkPrivilege` 的 Windows CI runner 上是一条真断言（判据 §2：一条在这里不可能失败的测试没有假装自己能）。

### 5.4 `_presentation` 普查覆盖了什么、明确没覆盖什么

`src/tools/presentation_census.rs`（`#[cfg(test)]`）。三条方向，**不对称是刻意的**，代码里有注释请后来者别去「统一」它们：

- **A（ghost 方向，严格）**：每个 `DISPLAY_NAMES` 键都必须被 `src/` 下某个 `const <IDENT> … = "<key>"` 声明匹上，
  **源码派生**而不是问一个运行时注册表实例——`BuiltinToolConfig::default()` 建的注册表**不含**
  `tool_search` / `memory_search` / `note_manage` / `subagent`（它们由别的子系统注册或需要记忆后端），
  用它做判据得到的是关于**夹具**的谓词而不是关于那张表的。两条**否定探针**（`"shell_exec"` / `"web_search"` 必须解析不出来）
  是「这条派生仍然抓得住旧派生抓到过的东西」的唯一凭据。
- **B（标签方向，宽松且**只会漏抓、不会误报**）**：`registered ∪ DISPLAY_NAMES 键` 里每一个都渲染出非空标签。
  它刻意用运行时注册表——覆盖不全，但那种窄只会让它**没抓到**，而 ghost 方向在同样的窄下会**误报**。
  配一条非空性探针（`display_name("__")` 确实渲染成空）证明这个谓词能开火。
- **C**：三个改文件内容的工具（`file_edit` / `file_write` / `apply_patch`）经**真注册表**端到端执行、
  断言 `_presentation` 键在场；`apply_patch` 的夹具经 `tokio::task_local` 的 `FsScope`（`src/tools/fs_scope.rs`）
  解析相对路径，**不是** `std::env::set_current_dir`（那会和这个二进制里其余每一条测试竞争）。

**明确记录在案的洞**（写在扫描器自己的 doc 里，不是这里发明的）：
① const 扫描证明的是「某个 const 持有这个字符串」，不是「这是一个工具名」——一个死键若恰好与某个无关的具名字符串 const 同拼，会通过；
② 并集仍然漏掉一个**既未在本配置注册、又没有显式标签**的工具（那约 95 个走 humanize 的之一）；
③ 那约 95 个 humanize 标签**没有人审过**——一个新工具的自动标签读起来难看也照样发货。
这条是刻意换来的：把 95 个名字列进一张豁免表，就是把这个模块存在的理由原样搬了个家。

### 5.5 ⚠️ `capability::census` 红了一条，**是从 main 继承的**

`src/capability/census.rs::every_installed_global_is_a_capability_slot` 在分支起点 `40a3579a8` 就是红的，
差**恰好一条**——这既由推导预测（Task 18 只新增了一个生产 `CapabilitySlot`，即
`thinker/prompt_size_registry.rs` 那个；`TEST_REGISTRY` 在 `#[cfg(test)]` 下被 `production_text` 剥掉）
也由**直接测量**证实（2026-09-07 在 `40a3579a8` 上真跑的失败集合里就有它）。

**这个字面量做的是相对 +1（48 → 49），刻意不去「修」成 50。** 把它调绿会让这道闸开始撒谎——
main 上某处加了一个生产 `CapabilitySlot` 而没有推这个数字，而闸现在说的是实话。
**关闭它的办法是找出那个没入账的句柄，不是编辑这个数字**（判据 §2 / §17）。
断言消息自己也这么说，因为**失败的断言不是在 commit message 里被读到的**。

### 5.6 其他仍然开着的东西

- **`trace.tool_output` 还没有客户端。** 它是 Phase B/C 要接的读面。
  `context.breakdown` **已有一个**（2026-09-11，Phase B B7：`/context` 覆盖层），它的客户端在调 `reconcile`
  **之前**先从实时 `ContextGauge` 填上了 `provider_reported`（§3.2）——那句要求写在这里、写在 wire 类型上、
  也写在 `reconcile` 上，三处都在对一个当时不存在的客户端说话，而**没有一处说得出应该由谁来做**。
  ⚠️ 「两个新 RPC 都还没有客户端」这句话曾同时住在**四个**地方（CLAUDE.md 路由行、§5.1、本行、本文顶部横幅）；
  B7 改了前两处就以为改完了，剩下两处由 B8 的扫查找到。一个事实住在四个地方时，
  **数出来的那个数目本身就是要被证伪的东西**（判据 §1 / §6）。
- **drain 的错误臂无条件丢弃 `presentation`**（`event_drain.rs`），零生产者：三个错误调用点全都传 `result: None`，
  而 `presentation` 是从 `result` 派生的，所以 error 有值时 presentation 结构性为 `None`。
  ⚠️ 更窄也更准的说法：一个**跑了但返回 `success: false`** 的工具走的是**成功**分支（`Some(&output)`、`error: None`），
  所以 `Unavailable::ToolFailed` 在 `tool_end` 上**已经可达**。错误臂只服务 harness 级失败（工具压根没跑、被护栏拦下、或中止），
  那里根本没有 `ToolOutput`，也就没有 diff 可丢。**重访条件**：哪天某条 harness 错误路径开始携带 `ToolOutput`。
- **`ConnectionFailure::Timeout` 是零生产者、五消费者**（Panel 侧五处 match 臂）。本轮保留了这个变体并附上诚实的
  「今天没有路径产出它」注释，**没有**写计划里建议的那句点名产出路径的注释——那句会是假的。
  Phase C（连接 UX）必须二选一：把真实超时分类进它，或连同那五处一起 CUT。
- **`plugins/diff-viewer` 的 CUT 只记了一条笔记，没有执行**：`plugins/` 是指向兄弟仓 Aleph-plugins 的 git submodule，
  从这个 worktree 里删要么对上游毫无作用、要么弄坏 submodule 指针，而 `include_dir!` 在编译期嵌入那棵树。
  CUT 晚一轮，落在它该落的那个仓里。
- **`ToolGroup::headline` 无条件数 `rows.len()`**，而它的时长求和只算终态行（§4）。Phase A 不绘制 headline，
  措辞由 Phase B/C 第一次画它时决定。
- **`src/tools/result_processing.rs` 那处不是 rustfmt-clean 的行**（`hoist_presentation` 的畸形载荷断言，`:998`）
  在本分支上被**两个不同的实施者各拒绝过一次**，理由相同且正确：为一个装饰性收益去重排别人的活行会招来冲突。
  它由本轮末尾一次格式化清理关闭（提交 `0834d7dae`，与另外九个文件一起）。两位实施者都称它「pre-existing」，
  而它其实只是「不是我的」：在分支起点 `40a3579a8` 上它是干净的，所以归属本分支的更早一个任务。
  **「pre-existing」不报出它对照的那个基线时不成立**——同一句话在「相对我的任务」和「相对 main」两种读法下真假相反。
  留下的规则是：单跑 `rustfmt --edition 2021 <file>`，**不要** `cargo fmt -p alephcore`
  ——它跟着 mod 树走，会把树里既有的漂移一并卷进你的改动（[FEATURE_LOCATOR 附录 C.6](FEATURE_LOCATOR.md)）。

---

## 6. 改这一层之前 / Working notes

- **「diff 没显示出来」先问是哪一段断了**：工具挂没挂 `_presentation`（普查 C 方向）· hoist 有没有跑
  （`apply_layer_two`）· 哪条腿（实时帧 vs `trace.by_runs` 重放）· 还是**根本没有渲染器**（§5.1，今天最可能的答案）。
- **要给 `Presentation` / `FileChange` / `Hunk` / `HunkLine` 加字段**：`mask_presentation` 会编译不过。
  那是设计好的机制——把新字段**加进那个走查**，不要加 `..`。
- **要给 `Unavailable` 加成员**：先答两句——「现有的哪一个都不是真的，为什么」，以及「**谁产它**」。
  第二句答不出就是下一个 `Encoding`（写下来、没有生产者、一年后被删）。
  加了之后 `shared/ui_logic` 的穷尽 match 编译不过，那是第二道机制。
- **别在 handler 里「统一」脱敏**：两侧不对称是自洽的（§2.2），两个方向都明令禁止。
  ⚠️ 但这句**不豁免一条把工具 JSON 原样返回的新面**：`tools.invoke` 就是那样一条
  （不过 `apply_layer_two`，所以 `_presentation` 还挂在对象上），它在自己的 handler 里调
  **同一个** `mask_presentation`。脱敏的面现在是**三个**，普查是 `mask_presentation` 的调用点，不是这句散文。
- **别把 `context.breakdown` 改成读时重新派生**——那正是它存在的理由的反面（§3.2）。
- **别调 `capability::census` 的那个数字**（§5.5）。
- **数字带谓词**：本文里每一个计数都写了它数的是什么、以及测于哪个 commit。复述之前重数一遍，包括重数我的
  （附录 C.1）。
