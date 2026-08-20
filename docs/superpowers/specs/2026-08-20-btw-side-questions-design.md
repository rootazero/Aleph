# `/btw` 侧问 (Side Questions) — 设计文档

> 2026-08-20 · 对标参考项目 `pi-btw`（`/Volumes/TBU4/Github/pi-btw`，609 行单文件 TS 扩展，Apache-2.0）
> 状态：设计已裁定，待实现。实现计划见同名 plan 文档。

---

## 0. 一句话

`/btw <问题>` 在主 agent 继续工作的同时回答一个侧问：**只读、独立并发、不进主上下文窗口**，
答案默认永不回流主会话，但留一个**显式**提升动作。

---

## 1. 背景：Aleph 已经有 `/btw`，而它答不了它存在要答的那个问题

`/btw` 不是新功能。它在仓里已有实现：

| 锚点 | 内容 |
|---|---|
| `src/gateway/inbound_router/command_handler.rs:51` | `SpecialSlash::Btw { body }` 变体 |
| `src/gateway/inbound_router/command_handler.rs:67` | `classify_special_slash` 的 `"btw"` 臂 |
| `src/gateway/inbound_router/command_handler.rs:256` | `handle_btw` —— 建 `SessionKey::ephemeral(agent_id)`，走普通执行路径 |
| `src/gateway/inbound_router/mod.rs:826` | 唯一派发点，排在统一 `CommandParser` 之前 |

它有**六个**结构性缺陷，每一个都不报错、不红测：

### 1.1 上下文继承：零（旗舰缺陷）

`SessionKey::ephemeral(agent_id)`（`src/routing/session_key.rs:194`）**每次调用生成新 UUID**。
于是侧会话是一个空会话：

```
/btw 刚才那个配置文件叫什么来着？
```

里的「刚才」指向一个**不存在的上下文**。这个功能的主场景（主 agent 跑了十分钟，随口问一句刚才发生的事）
结构上无法工作。参考项目 pi-btw 恰恰把「seed 主会话消息」当作它的核心设计。

### 1.2 只读约束：无

侧会话没有任何工具约束，落 `ExecTier::default() == Auto`
（`src/config/types/policies/exec_tier.rs:726` 的 `default_tier_is_auto`）。
`bash` / `file_write` / `file_edit` 全部可达。

**后果**：主 agent 正在 refactor 一棵树时，一次「顺口一问」可以并发改同一棵树。
pi-btw 用 `["read","grep","find","ls"]` 白名单挡住了这件事；Aleph 一道闸都没有。

### 1.3 侧线记忆：零

新 UUID ⇒ 每次 `/btw` 都失忆，追问无法续同一条 thread。pi-btw 保留 20 组 exchange。

### 1.4 面数：一（违 R6）

只有 channel 打字能到达。TUI / Panel / CLI 全无。推导写在 `inbound_router`（channel 专属模块）里，
其它两张脸**结构上够不到**。

### 1.5 可发现性：零

不在 `commands.list`（`src/gateway/handlers/commands.rs`）、不在 `/help` 输出、
**`docs/reference/FEATURE_LOCATOR.md` 零命中**。
只有读过 `command_handler.rs` 的人知道它存在。

### 1.6 磁盘泄漏，且被一条注释掩盖

`src/routing/session_key.rs:76` 的注释逐字写着：

```rust
/// Ephemeral session (no persistence)
Ephemeral { agent_id: String, ephemeral_id: String },
```

而 `src/gateway/session_store/file_backend/mod.rs:343` 把 `session_type: "ephemeral"`
写进 `SessionMetadata` —— **它是持久化的**，且全仓**没有任何清扫者**（grep 无命中）。

这是判据清单 §0 那条的教科书形态：*「同一事实的两份表述，只改一份就是静默说谎——而注释正是说谎的那一方」*。
真正的代价不是磁盘：**这条注释让每一个读到它的人都不去写清扫器**。

今天每问一次 `/btw`，磁盘上多一个永远不会被任何面读到的会话目录。

### 1.7 唯一已经对的那一半

`SessionKey::Ephemeral` ≠ 主 key ⇒ `busy_queue`（`src/gateway/busy_queue/mod.rs`，按 session 分 FIFO 车道）
给它**独立车道** ⇒ 主 agent 在跑时侧问立即执行。这一半保留不动。

---

## 2. 对比分析：pi-btw ↔ Aleph

### 2.1 能力对照

| 维度 | pi-btw | Aleph 现状 | 目标 |
|---|---|---|---|
| 上下文继承 | 主会话 messages 写进子会话 journal，走官方 restore 路径 | 空会话 | 有界 Fork + 增量补种 |
| 只读约束 | 4 工具名字白名单 | 无（`Auto` 档） | `ExecTier::Plan` 天花板 |
| 侧线记忆 | 20 组，内存 | 零 | 稳定 key，`/new` 清空 |
| 面数 | 1（TUI） | 1（channel） | 3（channel + TUI + CLI 共用 `agent.run`） |
| 可发现性 | pi 命令表 | 零 | 命令注册表 ⇒ 三处发现面白拿 |
| 并发 | pi 立即执行 extension command | ✅ 已对 | 不动 |
| 提升进主会话 | 永不 | 不适用 | 默认永不 + 显式动作 |

### 2.2 关键洞察：pi 手搓的三个原语，Aleph 都已有且更强

**这是「连线优先」的全部答案 —— 不该新写任何一个。**

| pi-btw 手搓 | Aleph 已有 | 锚点 | Aleph 强在哪 |
|---|---|---|---|
| `buildSessionContext + convertToLlm` 写 journal | `SpawnContext::Fork { turns }` + `fork::snapshot` / `fork::seed` | `src/agents/types.rs:141`；`src/agents/subagent_spawner/fork.rs:327` | 逐字保真**且带前缀缓存温度**；`ForkSource = Arc<Vec<_>>` 已解决扇出时 K× cache write；`SessionForked` provenance marker 让既有读者白亮 |
| `tools: ["read","grep","find","ls"]` | `ExecTier::Plan` | `src/config/types/policies/exec_tier.rs` | 按**工具声明的元数据**判而非名字 ⇒ 新工具自动被覆盖、未知工具 fail-closed；且是 `effective_permission` **rung 0 地板**，一条 `"bash" = "allow"` 掀不翻 |
| `SessionManager.inMemory()` | `SessionKey::Ephemeral` | `src/routing/session_key.rs:76` | 已在用（唯一已对的那一半） |

**结论：Aleph 不缺能力，缺的是这三个原语和 `/btw` 之间的三根线。**

### 2.3 刻意不移植

- **TUI overlay 的具体实现**：pi-btw 的 overlay 是 `pi-tui` 组件树，Aleph TUI 是 ratatui，
  按 `interfaces/tui/src/tui/approval.rs` 的既有模态形态重建，不照搬组件结构。
- **20 组硬上限**：Aleph 侧会话是真会话，压缩机制天然适用；上限改由侧会话自己的 context budget 管，
  不再手搓一个第二真源。

---

## 3. 已裁定的三个产品决策（2026-08-20，用户裁定）

| 议题 | 裁定 | 被否掉的选项 |
|---|---|---|
| **面数** | Core + Channel + TUI overlay。Panel 列 backlog 并在文档里写成**已声明的边界** | 「只修 channel 不扩面」；「全四面含 Panel」 |
| **上下文深度** | **有界** Fork，`turns: Some(10)`，可配 | 无界 Fork（长 run 里每次侧问付全额 prefix write）；Summary（没有产出这份摘要的人，需额外一次 LLM 调用，更贵更慢） |
| **提升** | 永不自动进主会话 + 一个**显式**提升动作 | 严格永不（用户只能手抄）；自动注入摘要（直接消灭 `/btw` 的存在理由） |

第三条的判据是 §4.11 round-12 那条：*「选错的方向必须是"要出声地要求"而不是"默认就给"」*。

---

## 4. 架构

### 4.1 btw 是 turn-kind，不是第六根会话旋钮

CLAUDE.md 的会话旋钮表五根共用一套机制：precedence「请求 > 会话 > 全局」，
且**请求携带的值会被盖回会话**（选择活过它所在的那一轮）。

btw 的语义恰好相反：它**只作用于这一次调用**，绝不改变会话状态。
塞进旋钮表就会继承「盖回会话」，等于一次侧问把主会话**永久**降到 `Plan` 档。

因此 btw **不进** `turn_*.rs` 孪生解析器族，**不进** `sessions.patch` 的 `knob_validators()`，
**不进** `session_snapshot.rs` 的解码。加旋钮的五步清单对它整条不适用。

### 4.2 推导落点：`stamp_slash_mode`

单一源：`ExecutionEngine::stamp_slash_mode`（`src/gateway/execution_engine/slash_command.rs:62`）。

它已经是三张脸共用的咽喉：

| 调用点 | 服务的面 |
|---|---|
| `src/bin/aleph-server/server_init.rs:272` | `agent.run` — TUI / CLI |
| `src/bin/aleph-server/server_init.rs:466` | `chat.send` — Panel |
| `src/gateway/execution_engine/execute.rs:317` | channel（inbound router） |

**它排在繁忙闸之前，这一点是载荷性的。**

判据 §5.24 ②：*「一个在闸之后才算出来的事实，答不了闸的问题——而两者都"有代码"，所以看起来是接好的」*。
`carries_more_than_text`（`src/gateway/execution_engine/steering.rs:207`）读
`SLASH_COMMAND_MODE_KEY` 判断「这条消息不能折进正在跑的兄弟」。
若 btw 在闸之后才算出来，主 agent 在跑时**每一条 `/btw` 都会被折成 steering 文本**
—— 静默进主上下文、永不作为侧问执行，**恰好是 `/btw` 存在要防的那件事**，且零报错零红测。
（`/model` 在 TUI 上整类失效正是这个形状。）

新增的唯一谓词：`BtwTurn::resolve(input) -> Option<BtwTurn>`，纯函数，单一源。
三张脸各自把结果喂给同一个 `apply()`。

### 4.3 三个效果

| 效果 | 接到哪 | 备注 |
|---|---|---|
| 上下文继承 | `SpawnContext::Fork { turns: Some(N) }` | 见 §5.1 增量补种 |
| 只读约束 | `ExecTier::Plan` 作为**天花板** | 见 §4.4 |
| 独立并发 | 已有（`SessionKey::Ephemeral` + `busy_queue` 分车道） | 不动 |

### 4.4 只读：天花板复合，不是新白名单

`exec_tier.rs:243` 已有 *「The stricter of two tiers — the composition rule for a ceiling」*，
其 doc 明写 *「may only ever tighten, so a ceiling cannot accidentally grant」*。
既有的 channel clamp 与 non-operator ceiling 全部经这条规则复合。

btw 的 `Plan` 是**又一个经同一条规则复合的天花板**，不是新机制。
好处：`Full` 档的用户发 `/btw` 同样落 `Plan`；而 btw 本身**不能**放宽任何东西
（复合规则在类型上就不允许）。

#### 4.4.1 `Plan` 的两个 carve-out 要按 btw 撤销

`PLAN_REACHABLE_TOOLS = ["scratchpad", "subagent"]`（`exec_tier.rs:131`）对规划档是对的，对 btw 是错的：

- **`scratchpad` 必须挡掉** —— 它写的是**主会话的执行清单文件**。
  一个只读侧问能改主 agent 正在执行的计划，比原 bug 更贵。
- **`subagent` 必须挡掉** —— 侧问派生的后台子代理会**活过侧会话**，
  而侧会话没有任何面能枚举它。同族于 §4.13b 那条
  *「纯内存的注册表在进程消失后不是空了，是撒谎了」*。

实现要求：这两条是 btw 天花板**同批声明**的 carve-out 撤销，**读 `PLAN_REACHABLE_TOOLS` 同一张表**
（不各持一份清单 —— 那正是这张表要防的错误挪高一层）。
该常量长出第三个成员时，btw 必须**编译期或守卫级**被迫回答「这一个要不要撤销」。

---

## 5. 生命周期

### 5.1 侧线记忆：稳定 key + **增量** re-fork

侧会话 key 由主会话 key **确定性派生**：一个主会话恰好一个侧会话。
记忆、复用、目录收敛三件事一次解决。

**派生规则（精确）**：`ephemeral_id = "btw-" + short_hash(主 key 的完整 key_string)`，
其中 `key_string` **含 epoch 后缀**（`SessionKey::append_epoch`，`session_key.rs:223`）。

含 epoch 是有意的，它买到两件事：

1. **`/new` 天然换一条侧线**。epoch 一 bump，派生出的侧 key 就变了 ⇒ 新侧会话为空
   ⇒ pi README 承诺的「`/new` 即清空」按构造成立，不靠任何人记得去清。
2. **旧侧会话变得不可寻址** ⇒ §5.2 的退休钩子只需负责**删掉它**，
   不需要同时负责「让它不再被读到」。两个职责分开，退休钩子漏跑的最坏后果是磁盘残留，
   **不是**一条串台的侧线。

⚠️ 派生**必须是单一源**（一个 `fn btw_key_for(main: &SessionKey) -> SessionKey`）。
写它的路径与读它的路径必须是同一个函数 —— 判据 §5 那条
*「`~/.aleph` 下的任何路径，写它的和读它的必须是同一个函数，不是"看起来一样的两段代码"」*
在这里同样成立：两处各拼一遍 hash，在 epoch=0 时逐字节相同，**本机永远测不出来**。

**陷阱**（§5.13 判据：*「两条投影同时喂同一个 append-only 状态，就会翻倍」*）：
key 稳定后若每次 `/btw` 都重新 `fork::seed` 整份主转录，侧会话变成
`seed₁ + Q1A1 + seed₁seed₂ + Q2A2 …`。

pi-btw 的做法是**只 seed 一次**（`ensureBtwSession` 里 `if (subSession) return subSession`），
代价是第二次之后侧 agent 看到的主会话**是十分钟前的** —— 而
「主 agent 跑了十分钟之后随口问一句刚才的事」正是本功能的主场景。

**Aleph 做法：增量补种。** 侧会话上记一个「已 seed 到主会话哪个事件」的游标，
每次 `/btw` 只补**闭合于游标之后**的那几轮。

- 追加式 ⇒ 前缀缓存全程温热（整份重 seed 会把整段前缀重键，正是缓存存在要防的）
- 游标**只有一个写者**（seed 那一步自己写），不另立第二个真源
- 这**不是超前设计**：不做它，功能答不了它存在要答的那个问题

### 5.2 泄漏修复（三部分）

1. **注释改成真话**（`session_key.rs:76`）—— 它是说谎的那一方
2. **稳定 key 把 N 个目录塌成 1 个**（每主会话一个，不是每问一个）
3. **`/new` 退休侧会话** —— 走已有钩子
   `crate::gateway::continuation_lifecycle::terminate_session_continuations`
   （`src/gateway/continuation_lifecycle.rs:95`，`handle_new_session` 已在调用它）。
   与 pi README 承诺的「`/new`、重启即清空」对齐。

---

## 6. 三张脸

| 面 | 现状 | 要做什么 |
|---|---|---|
| **Channel** | `handle_btw` 可用但推导错 | 推导搬走后只剩投递；回执**标记成侧答**，与主回复视觉可分 |
| **TUI** | 无 `/btw` | overlay + **一个前置修复**（§6.1） |
| **CLI** | 无 | 白拿（与 TUI 共用 `agent.run` 入口 + `stamp_slash_mode`） |
| **Panel** | 无 | **backlog**，在 FEATURE_LOCATOR 写成「已声明的边界」而非遗漏 |

### 6.1 TUI 前置修复（载荷性，必须先做）

`interfaces/tui/src/tui/app/mod.rs:662` 的注释自陈：

> The TUI subscribes to no topics, so the gateway's `should_receive` gives it both.

TUI **不订阅任何 topic ⇒ 收全部帧**。而 `interfaces/tui/src/tui/app/events.rs` 里
`session_key` **只用于澄清对话框**（260–295 行），
转录帧（`ResponseChunk` / `TextEmitted`）**没有任何按会话的客户端路由**。

⇒ 接 overlay 之前，btw 的输出会直接落进 TUI 主转录，
**恰好污染它存在要保持干净的那块屏幕**。

**这更可能是既有缺陷而非本轮引入**：Panel 在 §6.9 ① 修过同一形状
（`resolve_target` 对认不出归属的帧回退到当前对话），TUI 从没收到过那次修复。
任何 background / cron / 委派 run 的帧今天可能已经在污染 TUI 主转录。

**待探明（实现第一步）**：真机探一次，确认服务端 `should_receive` 有没有替它兜住。

- 兜住了 ⇒ TUI overlay 工作量小一截，本轮按单一 slice 推进
- 没兜住 ⇒ **它是一个独立于 btw 的既有 P1，拆成本轮的第一个独立 slice 先修先合**
  （理由：它今天就在损害 background / cron / 委派 run 的 TUI 体验，
  而那批用户没有在等 btw；把一个既有 P1 的修复绑在一个新功能的交付上，
  是让它多等一轮）

**btw 的设计不得建立在「它兜住了」这个假设上**
（判据：*「能不能按归属裁决，取决于生产者有没有把归属留下来」*）。

### 6.2 Channel 投递的保序决定（已裁定）

btw 回执与主回复去同一个 conversation，所以「btw 回执要不要排在主会话已排队的回复之前」
必须是一个**被写下来的决定**，不能是默认行为的副产品
（判据 §5.6：*「保序只在慢路径上实现等于没有」*）。

**裁定：btw 回执不参与主线保序，直接投递。**

理由：保序保护的是**同一条对话线的因果顺序** —— 主会话的回复 A、B 必须按 A、B 到达，
因为 B 可能引用 A。btw 回执**不在那条因果链上**：它既不引用主回复，也不被主回复引用，
把它塞进主线队列只会让一个「随口问」等在一串它无关的消息后面，
而「立即得到答案」正是这个功能的全部价值。

代价（明说）：一次 `/btw` 的答案**可能插在两条主回复之间**到达。
这在 channel 上视觉可辨（§6 的「侧答标记」承担这件事），且比等待更符合意图。

---

## 7. 提升 (promote)

默认严格隔离。提升是**显式动作**：channel `/btw promote`，TUI overlay 一个键。

注入形态必须过 `src/thinker/nudges.rs` 那道单一源。

判据 §2.20 ③：*「`User` 角色不等于"用户说的"」* —— 逐字保真只跳过摘要，
其余骑在 `User` 角色上的合成文本会被当成用户原话整条回贴，最贵的一条能吃掉 20k 用户预算。

⇒ 提升进去的答案必须**同批**告诉 `is_synthetic_reminder`（`nudges.rs:317`）它是什么。
`user_interjection_note`（`nudges.rs:157`）是既有的「用同一道 fence 包真实用户 steering」的形态，
提升的答案**不是**用户原话，所以不能直接复用它 —— 需要一个并列的、被分类器认得的载体。

---

## 8. 熵减（删除清单）

| 删什么 | 位置 | 为什么 |
|---|---|---|
| `SpecialSlash::Btw` 变体 | `command_handler.rs:51` | 推导搬到共用咽喉后是死代码 |
| `classify_special_slash` 的 `"btw"` 臂 | `command_handler.rs:67` | 同上 |
| `handle_btw` | `command_handler.rs:256` | 同上 |
| 唯一派发点 | `inbound_router/mod.rs:826` | 同上 |
| 相关 btw 单测（4 条 classify 测试） | `command_handler.rs:591–630` | 随被测函数一起 |
| 那条撒谎的注释 | `session_key.rs:76` | §1.6 |

**`classify_special_slash` 保留 `Help` / `Stop` 两臂**（它们确实是 inbound-only）。

**`/btw` 进命令注册表** ⇒ `commands.list` / TUI 命令树 / `/help` 三处发现面白拿，
**不新增任何登记表**。

---

## 9. 守卫

按 Aleph 的规矩：**会按名字红**、**派生不列举**、**写完手动破坏一次看它红且点得出行号**。

| # | 守卫 | 形态 | 防的是 |
|---|---|---|---|
| 1 | **效果到达断言** | **同进程**集成测试：跑真 btw 回合，断言一个 mutating 工具被拒 | §0 `EXEC_WORKSPACE` 教训逐字适用 —— 沙箱测试手搓、工具测试跑假沙箱，**只有把两半放进同一个进程才看得见**。断言「调用发生了」是产地不是连线 |
| 2 | **三面共用一个推导** | census，从 `stamp_slash_mode` 的**调用点派生** | *「守卫按名字列举成员，成员集合增长时它不会知道」* —— 不写三个字面量 |
| 3 | **不翻倍** | 连续两次 `/btw` 后断言侧转录里主会话前缀出现**且仅出现一次** | §5.1 的增量补种 |
| 4 | **carve-out 撤销生效** | 断言 `scratchpad` / `subagent` 在 btw 回合下被拒；且从 `PLAN_REACHABLE_TOOLS` **派生**成员集 | §4.4.1；该常量长出第三个成员时必须红 |
| 5 | **提升是唯一出口** | 源码级：btw 路径不得写主会话 key | §7 |
| 6 | **btw 不是旋钮** | 源码级：btw 不出现在 `knob_validators()` / `session_snapshot` 解码 | §4.1，防后来者「顺手补齐」把它变成会盖回会话的旋钮 |

守卫 1 与 4 **必须各手动破坏一次**并记录它红的行号。
判据：*「一条没被证伪过的守卫不算守卫」*。

⚠️ 源码级守卫的分隔符**不要锚行首行尾**（CRLF 检出上 `\n#[cfg(test)]\n` 永不匹配，
守卫会开始扫自己的测试模块并被断言字符串里的字面量满足）。先 `.replace('\r', "")` 再 split。

---

## 10. 刻意不做（Deliberately Not Doing）

| 不做什么 | 理由 |
|---|---|
| **Panel 面** | 用户裁定。需新写 overlay 组件 + 帧路由（`resolve_target` 现会把 ephemeral 会话当 background run），约为 TUI 面两倍工作量，且要过 `disposed_reads` / theme / 手机端三套约束 |
| **`/btw` 多轮并发** | 一次一个侧问；第二个**直接拒**而不排队。排队会得到一个「问了但十分钟后才答」的面，比拒更差 |
| **侧会话跨进程恢复** | 重启即清空，与 pi 一致。跨进程恢复要求侧会话进正式持久化面，与「不留痕」的产品意图冲突 |
| **btw 用更便宜的模型** | 侧问要看懂主转录，降档会得到一个看不懂上下文的答案。留作后续可配项，本轮不做 |
| **自动提升 / 自动摘要回流** | 用户裁定否决。直接消灭 `/btw` 的存在理由 |

---

## 11. 验证

最小可信验证集（判据清单 §10，**五条，不是一条**）：

```
cargo test -p alephcore --lib --no-run
cargo test -p alephcore --features test-helpers --test '*' --no-run
cargo test -p aleph-panel --lib --no-run
cargo clippy --all-targets
cargo test -p aleph-tui -p aleph-cli          # 本轮改 TUI，必跑
```

⚠️ 最小验证集**不覆盖客户端 crate**，而本轮的 wire 契约两半正好住在两个 crate
（`aleph-tui` ↔ `alephcore`）—— 这是 `aleph workspace create` 与 TUI `agent.run`
两次翻车的同一形状。第五条不可省。

真机 QA：`qa/channels/run.sh`（改通道接线）。TUI 面用 pty 驱动
（见记忆 `reference-realmachine-qa-rig-additions`）。

---

## 12. 后续文档更新

实现完成后同批更新：

- `docs/reference/FEATURE_LOCATOR.md` —— **新增章节**（今天零命中），含 Panel 面「已声明的边界」
- `docs/reference/GATEWAY.md` —— 推导落点与三面共用咽喉
- `docs/reference/SECURITY.md` —— btw 的 `Plan` 天花板与两个 carve-out 撤销
- `CLAUDE.md` 判据清单 —— 若本轮抓到新的跨子系统形状
