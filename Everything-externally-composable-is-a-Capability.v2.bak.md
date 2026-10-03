# Everything externally composable is a Capability

> **Aleph Zahir 总纲 · v3（2026-10-02 修订）**
> v2 → v3 变更：正式命名 Zahir、基于 Pi Durable 实证修订定位与架构、补充 Aleph 现状清点

---

## 〇、先读这一节：怎么在重构会话里用这份文档

**这份文档的性质**：它是**总纲**，不是 spec，也不是实现计划。它规定方向、边界和判据；具体的类型、文件、迁移步骤由新一轮的 brainstorming → spec → plan 产出。

**开工前置条件（按顺序）**

1. `plugin-scope-round`（插件作用域 + CC 兼容 + MCP 面）**已合并进 main**。那一轮在插件维度做了"效果作用域 + 可撤销注册"，正是本总纲要推广的第一块——没合并就开工，新会话看到的是缺了这一块的旧代码。
2. **第一步是清点，不是写 spec**（见 §十四）。清点结果决定这是一次"整理"还是"真正升一级"，也决定范围。
3. 标注为 `[待核实]` 的关于 dsh / pi 的断言，写进 spec 之前先对照 `/Volumes/TBU4/Github/` 下的源码核实。v1 中它们被标成 `[KNOWN]`，但没有人拿源码对过。
4. CLAUDE.md 写着"对照表已做完：pi · deepseek-harness"。本轮**修订**那两份对照，不重做。

**v2 相对 v1 改了什么**

| 类别 | 改动 |
|---|---|
| 坚持（保留并加强） | ① 能力语义与插件装配分离 ② 所有权 + 可撤销副作用推广到插件之外 ③ 协议只是能力的投影——并明确这三条**真正的收益是让一类缺陷从结构上写不出来** |
| 推回（修改了 v1 的主张） | ① 不是白纸：收敛现有注册表，不新建平行内核 ② 依赖图不在会话中途增删工具 ③ Scope 只建有消费者的级别 ④ 能力分类区分"种类"与"关系" ⑤ Resolver 不得按消息内容挑能力 ⑥ 优先级表按 Aleph 现状重排 |
| 新增 | 红线对照（§十二）、v1 未覆盖的四件难事（§十一）、成功判据与失败形态（§十三）、第一步清点清单（§十四）、**ACP / MCP 双向性与现有 ACP 的关系（§12.3）** |

---

## 一、Aleph 的定位：它不是 dsh，也不是 Pi

Aleph 最适合的定位不是"另一个 Agent Harness"，而是：

> **一个以 Capability 为基本组合单位、协议无关、宿主无关的通用 Agent Runtime。**

```
                    ┌─────────────────────┐
                    │       Aleph         │
                    │   General Agent     │
                    │      Runtime        │
                    └──────────┬──────────┘
                               │
              Everything externally composable
                     is a Capability
                               │
         ┌─────────────────────┼─────────────────────┐
         ↓                     ↓                     ↓
      Tools                Resources              Services
         ↓                     ↓                     ↓
 Browser / Shell        Files / Web / DB       Memory / LLM
 Code / GUI             Documents / State      Policy

         ┌─────────────────────┼─────────────────────┐
         ↓                     ↓                     ↓
      Events                Tasks                  Agents
         ↓                     ↓                     ↓
 hooks / streams        background jobs       sub-agents / peers
```

Aleph 拥有自己的 Capability Model；dsh、Pi、Claude Code、Codex 等都只是消费 Aleph Capability 的**宿主**。

```
Aleph ≠ dsh plugin
Aleph ≠ Pi extension
Aleph ≠ CLI tool
Aleph ≠ MCP server
```

这些都是 Aleph 在不同生态中的**投影**。

---

## 二、从 dsh 学什么：所有权、依赖与副作用生命周期的制度化

以下关于 dsh 的描述来自 v1，**`[待核实]`**：

- dsh 自称 "no privileged core"：model adapter、tool registry、session、agent、agent-loop 都以 plugin / service 身份参与组合，经 services、typed events、reversible effects 组成运行时。
- `ctx.tools` / `ctx.llm` / `ctx.agents` 是 Service，插件经 `inject` 声明依赖；依赖未就绪插件不运行，必需服务消失时依赖它的插件自动卸载，服务恢复后可重载。
- `ctx.on()` / registry 注册 / `ctx.plugin()` 挂子插件等动作绑定到所属 fiber 的生命周期，unload 时自动撤销；timer / socket / watcher 可经 `ctx.effect()` 显式绑定 disposer。

dsh 真正先进的地方不是"插件多"，而是**把能力的所有权、依赖和副作用生命周期制度化了**：

```
Capability → Owner / Scope → Effects → Mount → Active → Unmount → 全部撤销
```

而不是：

```
register() register() register() ... 最后祈祷没人忘记 cleanup()
```

**为什么这对 Aleph 是真需求而不是品味**：Aleph 反复付账的几类缺陷，根子都是"谁拥有、谁清理"没有制度化——测试临时文件泄漏、进程级静态缓存被污染（`static GLOBS` 一类）、注册了却没有派发（判据 §7）、报成功却什么都没发生（判据 §11）。

**已有的第一块**：`plugin-scope-round` 在**插件**维度落地了效果作用域 + 可撤销注册（`ScopeKey` 一个谓词管多张脸）。本轮是把它**推广**到会话、子 agent、MCP 连接、浏览器会话等维度，从那份真实实现出发泛化，而不是从零构想。

---

## 三、不照搬 dsh：Plugin 是装配机制，Capability 才是语义

```
dsh:                              Aleph:
Plugin                            Capability
 ├── provides Service              ├── 模型可调用（Tool）
 ├── registers Tool                ├── 可读/可订阅（Resource）
 ├── listens Event                 ├── 持续产生事件（EventSource）
 └── owns Effects                  ├── 异步/后台工作（Task）
                                   ├── 可被调用的智能体（Agent）
                                   └── 运行时内部长期服务（Service）
```

**Plugin 是部署/装配机制；Capability 才是能力语义。** 这一点 Aleph 其实已经在隐式地做：浏览器双引擎背后是一套接口、provider 背后是 trait、沙箱背后是平台实现。本轮是给它一个正式的名字和一套统一的所有权规则。

```
Browser Capability ← Obscura / Chromium / WebView / Remote
Shell Capability   ← Local / Sandbox / SSH / Container
LLM Capability     ← OpenAI / Anthropic / DeepSeek / Local
Memory Capability  ← Working / Long-term / Vector / Graph
```

Agent 看到的是统一的 Capability，不知道也不应该知道底下是哪个实现。

---

## 四、从 Pi 学什么：极小核心 + 极强扩展面

以下关于 Pi 的描述来自 v1，**`[待核实]`**：

- Pi 定位 minimal terminal coding harness，把 workflow 功能放在 extensions / skills / prompt templates / packages。
- 默认工具极少（read / write / edit / bash），但有正式 extension API：注册 Tool / Command / Shortcut / Provider，监听并修改 tool call / tool result / agent 生命周期 / prompt，支持热重载。
- 有 package 系统（npm / Git / 本地路径），支持 RPC、JSON event stream、SDK embedding。
- MCP 不在核心，而在 extension / package 层。

对 Pi 最准确的描述是：**极小的 Agent Core + 极强的 Extension Surface + CLI / SDK / RPC 多种宿主方式**。

"Bash-only"不再是准确描述。Bash 的意义是 **Default primitive + Universal escape hatch**：核心少提供高级抽象，需要时允许用户经 Shell 自建。Aleph 吸收这个思想，但不复制 Pi 的 coding-agent 定位。

---

## 五、dsh / Pi / Aleph 对照

| 维度 | dsh | Pi | Aleph 怎么吸收 |
|---|---|---|---|
| 核心 | Plugin / Service Graph | Minimal Agent Core | Capability Kernel（**收敛现有注册表而成**，见 §六） |
| 依赖 | 强依赖声明 | 较弱、更自由 | 依赖声明 + **会话边界生效**（见 §九） |
| 生命周期 | 强、可逆 | 有 lifecycle，非依赖图 | 吸收 dsh，推广 `plugin-scope-round` 的实现 |
| Tool / Event | 一等公民 / 强类型可拦截 | 一等公民 / 事件丰富 | 吸收二者 |
| Scope | Context / Agent scope | session / package / project | Aleph Scope（**只建有消费者的级别**，见 §八） |
| 热重载 | 深度融入 runtime | extension reload | 吸收二者 |
| CLI / RPC / SDK | 有 | 很强 | 已有 `al` CLI 与 JSON-RPC Gateway，**扩展而非新建** |
| 极简性 | 较低 | 很高 | 核心保持小（R3 / R10） |
| 宿主依赖 | Cordis / Harness | Pi runtime | 不绑定任何一个 |

最好的 Aleph 不是两者折中，而是：**dsh 的 Runtime Discipline + Pi 的 Minimal Core + Aleph 自己的 Capability Model**。

---

## 六、Capability Kernel：收敛，不是新建

### 6.1 结构

```
┌────────────────────────────────────────────────────┐
│                  Aleph Agent Layer                 │
│   Agent Loop（src/harness/，R10 锁定）/ Sub-Agent   │
└─────────────────────────┬──────────────────────────┘
                          │  resolve / invoke / observe
┌─────────────────────────┴──────────────────────────┐
│                 Capability Kernel                  │
│  Registry · Scope · Lifecycle · Effect · Policy    │
│  Schema · Event · Cancellation · (Dependency)      │
└──────────────┬──────────────┬──────────────┬───────┘
           Tools          Resources       Services
          Tasks / Jobs     Events          Agents
```

Agent Loop 不直接拥有能力，只能：`resolve(capability) → invoke(capability) → observe(events)`。这与 R10「薄 Harness，笨循环」同向，并且**不需要改动 `src/harness/`**——Kernel 落在工具呈现层与运行时层，不进 harness。

### 6.2 ⚠️ 关键纠正：Aleph 不是白纸

v1 把 Kernel 的九个组件写成需要新建的东西。实际上 Aleph 里大多已有某种形态（具体名单由 §十四 的清点给出）：

| Kernel 组件 | Aleph 里已有的形态（待清点确认） |
|---|---|
| Registry | `ToolCatalog`、RPC `HandlerRegistry`、插件注册、skill 注册、MCP manager、**ACP harness manager（`src/acp/manager/`）**、provider 预设表…… |
| Scope / Lifecycle / Effect | `plugin-scope-round` 的 `ScopeKey` + 可撤销注册（插件维度） |
| Policy / Permission | 审批门、`[sandbox.command_policy]`、工具权限三层、principal / spend ledger |
| Schema | schemars |
| Event Bus | Gateway 事件、hooks |
| Cancellation / Timeout | 工具执行预算、取消路径 |

**所以本轮真正的工作不是"设计一个内核"，而是"把现有的 N 个注册表收敛成一个"。**

最大的风险是：新 Kernel 与旧注册表**并存**。那正是判据 §1「同一事实的两份表述」，也是这个仓库最贵的失败形态。硬规则：

- **每引入一个 Kernel 组件，同一轮里必须删掉它取代的旧注册表**；做不到删除就不引入。
- **衡量成功的指标是删掉了几个旧注册表，不是新建了几个模块。**

### 6.3 ⚠️ Capability Resolver 必须守住 R7

"Resolver 挡在 Agent Loop 前面"只允许两种解析方式：

1. **静态**：按配置 / 作用域 / 权限做的、不看消息内容的分区（与 `src/tools/scoped/` 的静态 `retain` 同性质）；
2. **模型发起**：如 `tool_search`——加载决策 100% 由模型做出。

**禁止**：Resolver 按用户消息内容挑选或过滤能力。那是 R7 明令禁止的意图路由，也违反 R10 的第二个"不"（不按意图过滤）。

---

## 七、Capability 的分类：区分"种类"与"关系"

v1 把七类并列：Tool / Resource / Service / EventSource / Task / Agent / Provider。**这把两种不同的东西混在了一起**：

- **种类（kind）**：这个能力对外呈现为什么——**谁来用、怎么用**。
- **关系（relation）**：这个能力由谁实现、依赖谁——**Provider 不是一种能力，而是"谁实现了谁"的关系**。

修订后：

```
Capability kind（对外呈现，面向消费者）
│
├── Tool         模型可以主动调用
├── Resource     Agent 可以读取 / 订阅
├── EventSource  持续产生事件
├── Task         异步 / 后台工作
└── Agent        可被调用的智能体

Capability relation（内部结构，面向运行时）
│
├── implements   Obscura implements Browser
│                Claude Code（经 ACP harness）implements Agent
│                某个外部 MCP server implements 一组 Tool / Resource
├── requires     BrowserTools requires Browser
└── owned-by     BrowserSession owned-by Session scope
```

**Service 的去留交给 spec 决定**：v1 中 Service（"运行时内部长期提供能力"）与 Provider 高度重叠。若 Service 只被运行时内部消费、从不对外呈现，它就不是"externally composable"，按本文标题不属于 Capability，而是某个 Capability 的实现细节。spec 必须给出一个**能让某个东西落在两边任一侧的判定问句**，而不是列举。

⚠️ 分类法一旦写进类型系统就很贵，所以要在 spec 阶段先较真，并用清点出的真实实例逐一试分类——分不进去的实例就是分类法的缺陷。

**浏览器示例（修订后）**：

```
Browser
├── Tool         navigate / click / type / screenshot
├── Resource     page / DOM / accessibility_tree / page_state
├── EventSource  page_loaded / network_request / navigation_changed / user_interaction
└── relations
    ├── implemented by  Obscura / Chromium（均经 crates/aleph-cdp）
    └── owned-by        BrowserSession
```

"上帝之手"、双向 IPC、鼠标事件拦截、网页空间状态，都只是 Browser Capability 的不同事件与操作面，不是塞进 Agent Loop 的特殊逻辑。

---

## 八、Scope：只建有消费者的级别

v1 提出六级：Global → Runtime → Session → Agent → Task → Turn。**这是 YAGNI 风险**。

**修订**：

- **首批只建今天就有消费者的级别**，预计是 **Runtime（含插件）/ Session / Agent** 三级。准确名单由 §十四 清点决定：哪一级找不到现有消费者，就不建。
- **其余级别按三次法则**：第三个真实需求出现时再加。
- **Turn Scope 特别警告**：v1 的例子是"Turn 级的临时 prompt 贡献"。这会直接碰 R9（智慧在 Prompt 中，但 prune-the-prompt）与 R10（harness 锁定），还会打破 prompt cache。**本轮不做**；若将来要做，必须先过 R9 的两把尺，并说明不进 `src/harness/`。

Scope 必须是**树**（子 Scope dispose 先于父 Scope），且**每个 Scope 必须能回答"它属于哪个 principal"**（见 §十一）。

---

## 九、Reversible Effects：可撤销的边界要说清

任何 Capability 注册行为都必须有 owner：

```
mount Tool        → owner = Agent
subscribe Event   → owner = Session
open Browser      → owner = BrowserSession
spawn Process     → owner = Task
connect MCP       → owner = Session

Owner.dispose() → unsubscribe / unregister / terminate / close / flush / release
```

### 9.1 ⚠️ 可撤销的边界

**能撤销的是"注册"，不是"世界上发生过的事"。**

| 可撤销（Kernel 负责） | 不可撤销（Kernel 只负责停止与记录） |
|---|---|
| 工具 / 事件订阅 / 路由的注册 | 已写出的文件、已发出的网络请求 |
| MCP 连接、浏览器会话、子进程句柄 | 已派生进程对外部世界造成的效果 |
| 临时目录（若由 Scope 创建并拥有） | 已发送的消息、已扣的花费 |

对不可撤销的那一列，dispose 只能做到"停止继续产生效果"。跨越不可逆边界之前要先盖**意图戳**（判据 §15：只记录"做完了"的机件分不出"没做"和"做了但没记上"）。

### 9.2 ⚠️ Rust 的现实：没有 async Drop

v1 写 `async fn dispose(self) -> Result<()>` 并视为理所当然。Cordis 依托 JS 单线程事件循环，在 Rust / tokio 里以下都是**更难**而不是更容易的地方，spec 必须逐条给出答案：

1. **没有 async Drop**：一个 Scope 被 drop 而没有显式 `dispose().await` 时怎么办？（同步兜底？泄漏告警？禁止隐式 drop？）
2. **dispose 失败**：返回 `Err` 时，父 Scope 是继续撤销兄弟效果，还是停止？失败的效果处于什么状态？（`Err` 不得被读作"已撤销"——判据 §8）
3. **撤销顺序**：同一 Scope 内多个效果的撤销顺序（通常逆注册序），以及跨 Scope 的顺序。
4. **与取消的交互**：一个正在执行的 invoke 所在的 Scope 被 dispose 时，invoke 如何被取消、结果归谁。

### 9.3 类型草图（仅示意，以 spec 为准）

```rust
pub struct EffectHandle {
    pub id: EffectId,
    pub scope: ScopeId,
}

pub struct Scope {
    pub id: ScopeId,
    pub parent: Option<ScopeId>,
    pub principal: PrincipalId,   // v2 新增：见 §十一
    pub effects: EffectSet,
}
```

```
Session.dispose() → Agent.dispose() → Task.dispose()
                  → Browser.dispose() → MCP.dispose() → Tool unregister()
```

各模块不再自己猜"什么时候应该清理"。

---

## 十、Dependency Graph：声明依赖，但不在会话中途增删工具

依赖声明本身有价值：

```
BrowserTool    requires: browser.page, browser.input, permission
CodeExecution  requires: filesystem, subprocess, sandbox
WebResearch    requires: browser, network, memory
```

### 10.1 ⚠️ 推回：v1 的"自动停用 / 自动重启用"

v1 主张：Browser 消失 → 依赖它的工具自动停用；恢复 → 自动重新启用。这与 Aleph 有三处冲突：

1. **R10 与 prompt cache**：会话中途工具集变化会打破 prompt cache，也会让模型困惑（它刚看见的工具消失了）。这不是 R10 允许的"不看消息内容的静态分区"。
2. **判据 §8**："依赖消失"只有资格说"我不知道"，不能被读作"这个工具不存在"。自动从工具列表里拿掉，恰好把"暂时不可用"答成了"没有"。
3. **Rust 实现成本**：见 §9.2。

### 10.2 修订后的语义

- **工具集在会话边界（或显式 reload）时确定**，会话中途不增删。
- 依赖不可用时，工具**仍在列表中**，调用返回**结构化的"暂不可用"错误**（带原因、带是否可重试），让模型看见并自愈——这是 A2「让模型看见并自愈错误 = 要」；而不是由 harness 替模型挑恢复策略（R10 第 5 不）。
- 依赖图的用途是：**启动 / reload 时的装配顺序、Scope dispose 时的撤销顺序、`doctor` 类诊断**。

---

## 十一、v1 没有覆盖的四件难事（spec 必须回答）

1. **按作用域的权限**。Aleph 有多用户 principal、`[policies.spend]` 花费账本、审批门、`allowed_users`。每个 Scope 属于哪个 principal？子 Scope 能否拿到比父 Scope 更大的权限？一个能力被投影到 MCP / CLI / Panel 时，调用方的 principal 从哪里来？
2. **重启之后 Scope 状态怎么重建**。对应 12-Factor 采纳条款 A3（状态可重建趋向纯 Reducer）与判据 §15（进程内存不是状态）。哪些 Scope 是持久的（Session），哪些随进程死亡（Task）？重启后一个"半撤销"的 Scope 是什么状态？
3. **能力 schema 跨 crate、跨 wire 的版本契约**。对应判据 §10：键集要放进两边都依赖的那个 crate 并用它构造响应。Capability descriptor 投影到 MCP / RPC / CLI JSONL 时，谁是真源？schema 演进时旧客户端读到什么？
4. **可撤销的边界**（§9.1）。

---

## 十二、协议是投影：一份描述，多个出口

```
                  Aleph Capability Kernel
                           │
              ┌────────────┼────────────┐
              ↓            ↓            ↓
             MCP          CLI          RPC
              │            │            │
        ┌─────┴────┐       │       external apps
        ↓          ↓       ↓
       dsh        Pi     Unix
```

Aleph 内部：`CapabilityDescriptor / CapabilityHandler / CapabilityScope / CapabilityEffect`。
MCP 只是：`Aleph Capability → MCP Tool / Resource / Prompt representation`。新协议 = 新增一个 `Aleph → NewProtocolAdapter`，不动 Core。

### 12.1 这才是"升一级"的真正来源

判据 §9「一个动词有几张脸，判据就要在每张脸上用同一个推导」是 Aleph **最常复发**的一类缺陷：工具面、RPC 面、客户端面、事件面各自漂移。

如果**每个能力只有一份 descriptor，每张脸都从它投影出来**，那么：

- §1「同一事实的两份表述」——在能力元数据上写不出来了；
- §7「两端完整而中间没线」——注册即派发，没有"注册了但派发表没那条臂"；
- §9「一个动词几张脸」——每张脸同一个推导；
- §11「报成功的 no-op」——修在执行者（Kernel 的 invoke 路径）上，一次覆盖全部能力。

**让判据清单里的几条从"要靠人记住"变成"写不出来"——这才是提升一个层级。**

### 12.2 红线对照

| 红线 | 本总纲的约束 |
|---|---|
| **R3** 核心轻量化 | Kernel 由现有注册表收敛而成，不引入新的重三方库；不新增 crate 依赖须在 spec 中逐项论证 |
| **R4** Interface 层无业务逻辑 | CLI / Panel / 宿主适配器只做投影与 I/O |
| **R7** LLM 主权 | Resolver 不按消息内容挑能力（§6.3）；依赖不可用交给模型处理（§10.2） |
| **R8** 工具即一切 | Capability 的管理操作本身也是能力，可被模型经工具调用 |
| **R9** 智慧在 Prompt 中 | 本轮不做 Turn 级 prompt 贡献（§八） |
| **R10** 薄 Harness | Kernel 不进 `src/harness/`；会话中途不增删工具（§10） |

### 12.3 ⚠️ ACP 与 MCP 都是双向的：别和现有 ACP 做重复

v1 只把 ACP / MCP 画成**对外出口**（Layer 4）。但 Aleph 今天已经在**另一个方向**上用着这两个协议。

**现有 ACP（2026-09-28 grep 核实，未通读 `src/acp/manager/`）**

- `src/acp/` 的方向是 **Aleph 作为 ACP client**：驱动外部 CLI agent（Claude Code / Codex / Gemini 的 `--acp`），支持 Tool mode（模型把它当工具调用）与 Agent mode（直接对话）。
- `src/acp/incoming.rs` 是这条连接的**反方向半边**：外部 agent 回头请求 Aleph 的 `fs/read_text_file` / `fs/write_text_file` / `session/request_permission` / `terminal/*`，文件操作限定在会话工作目录内，权限请求接入 Aleph 的审批门。
- 消费方：团队调度（`src/teams/dispatcher/acp_bridge.rs`）、Panel 设置里的 ACP harness 管理页（`interfaces/webchat/.../settings/acp_harnesses/`）、会话持久化（`src/acp/manager/persistence.rs`）。
- **Aleph 作为 ACP agent（被别人调用）的那一侧目前不存在**：`session/new` / `session/prompt` 只出现在 Aleph 发出请求的代码里，接收它们的只有测试用的 `src/acp/mock_server.rs`。

**所以 §十五 第 3 层的"dsh 经 ACP 调用 Aleph Agent"不是重复，而是补上缺的那一侧。** 两个协议在 Capability 模型里各落两处：

| 协议 | 方向 | 在 Capability 模型里的位置 | 现状 |
|---|---|---|---|
| ACP | Aleph → 外部 agent（client） | **Layer 1 / relation**：外部 agent 是 Agent 类能力，ACP adapter 是它的 `implements` 方 | 已有：`src/acp/` |
| ACP | 宿主 → Aleph（agent 侧） | **Layer 4**：把 Aleph 的 Agent 能力投影为 ACP agent | 缺 |
| MCP | Aleph → 外部 MCP server（client） | **Layer 1 / relation**：外部 server 是一组 Tool / Resource 的 `implements` 方 | 已有：`src/mcp/manager/` |
| MCP | 宿主 → Aleph（server 侧） | **Layer 4**：把 Aleph 的能力投影为 MCP Tool / Resource | 已有：`/mcp` 面（`plugin-scope-round`） |

**三条硬规则**

1. **每个协议的报文类型只有一份。** 将来做 ACP agent 侧，必须复用 `src/acp/protocol.rs` 的类型（`session/new` / `session/prompt` / `request_permission` 等），不写第二套 ACP 实现；MCP 同理。这与 CLAUDE.md 对 CDP 客户端"唯一真源是 `crates/aleph-cdp`"的禁令同形（判据 §1 / §10）。若两侧需要共享而当前类型位置不合适，**移动**它，不复制它。
2. **client 侧也要纳入 Capability 模型。** 现有 ACP harness manager 与 MCP manager 都是"按名字存放可调用东西"的注册表，属于 §6.2 要收敛的对象。只把 Layer 4 那一面接进 Kernel、把 client 侧留在外面，结果就是 v2 自己列为头号风险的"新 Kernel 与旧注册表并存"。
3. **同一套审批语义，两个方向一个推导。** 今天：外部 agent → `session/request_permission` → `incoming.rs` → Aleph 审批门。将来 Aleph 作为 ACP agent 时：Aleph 需要向宿主**发出** `request_permission`。这是**一条连接的两个方向**（判据 §9 的"脸"），"什么操作需要审批、审批结果怎么解读"必须共用一处推导，不得各写一份。

**现成的样板候选**：现有 ACP harness 同时挂在 Tool mode（模型工具面）、Agent mode（对话面）、团队调度（`acp_bridge`）、Panel 管理页四个面上——正是 §12.1 所说"一个能力、多个出口"的真实实例，适合作为 P0-c 的样板候选之一（以 §十四 清点结果为准）。

---

## 十三、成功判据与失败形态

**成功判据**

1. 删掉的旧注册表数量 ≥ 新引入的 Kernel 组件数量；
2. 至少一个动词的全部面（工具 / RPC / 客户端 / 事件 / MCP）由同一份 descriptor 投影，且有守卫在任一面漂移时变红；
3. 至少一类泄漏（测试临时文件 / 进程级缓存 / MCP 连接 / 浏览器会话）由 Scope dispose 结构性消除，且有变异测试证明守卫会红；
4. `src/harness/` 零改动。

**失败形态（出现即停下复盘）**

- 新 Kernel 与旧注册表并存超过一个任务（判据 §1）；
- 为"未来可能的需求"建了没有消费者的 Scope 级别或能力种类（YAGNI）；
- 任何按消息内容过滤能力的代码（R7）；
- 会话中途工具集发生变化；
- `dispose()` 的 `Err` 被当作"已撤销"消费（判据 §8）；
- 出现第二套 ACP 或 MCP 报文类型实现（§12.3 规则 1）；
- ACP / MCP 的 client 侧注册表在本轮结束时仍游离于 Capability 模型之外（§12.3 规则 2）。

---

## 十四、第一步：清点（在写 spec 之前）

派一个子代理，在合并后的 main 上产出以下清点表（每个数字带它的谓词与 commit——判据 §18）：

1. **注册表清点**：每一个"按名字存放可调用 / 可订阅东西"的结构——位置、键类型、写者、读者、是否有撤销路径。
2. **多面动词清点**：每个动词同时暴露在哪几张脸上（工具 / RPC / 客户端 / 事件 / MCP / CLI），各面的推导是否同一处。
3. **生命周期清点**：每一个需要清理的资源（进程、连接、临时目录、订阅、缓存）——谁创建、谁清理、清理路径是否存在、是否经 `plugin-scope-round` 的作用域机制。
4. **Scope 消费者清点**：Runtime / Session / Agent / Task / Turn 各级今天有没有真实消费者。
5. **双向协议面现状**：
   - **出口侧**：`/mcp` 面、`al` CLI、JSON-RPC Gateway 分别已经投影了哪些能力；
   - **入口侧**：ACP harness（`src/acp/`）与 MCP client（`src/mcp/manager/`）各接入了哪些外部能力、挂在哪几张脸上（工具面 / 对话面 / 团队调度 / Panel）；
   - **协议类型真源**：ACP / MCP 报文类型各在哪里定义、有几份；
   - **审批语义**：`incoming.rs` 的 `request_permission` 处理与 Aleph 审批门之间的推导链。

**读法**：注册表与多面动词如果只有少数几个，本轮是一次**整理**；如果有十几个，才是真正的**升一级**。范围按清点结果定。

---

## 十五、对外：三层兼容（dsh / Pi 同构）

| 层 | dsh | Pi | 性质 |
|---|---|---|---|
| 1 · 标准协议 | Aleph MCP Server → dsh MCP client → `ctx.tools` | Pi MCP extension → Aleph MCP Server | 零专用代码即可工作 |
| 2 · 原生适配器 | `dsh-plugin-aleph`（session / lifecycle / permissions / events / UI） | `@aleph/pi-extension`（registerTool / registerCommand / on(tool_call) / lifecycle） | 深度集成 |
| 3 · Agent 级 | ACP：dsh 调用 Aleph Agent | Pi → `al` CLI（NDJSON / JSON） | Agent 互操作 / 通用兜底 |

三种关系要分开：

```
宿主 → Aleph Tool        = MCP
宿主 → Aleph Agent       = ACP / Agent protocol
Aleph ↔ 宿主 internals   = Native adapter
```

⚠️ 第 3 层的 ACP 指 **Aleph 作为 ACP agent 被调用**——这一侧今天不存在；Aleph 现有的 `src/acp/` 是**反方向**（Aleph 作为 client 驱动外部 agent）。两者的关系、以及"报文类型只有一份"的规则见 §12.3。

**CLI 是 Universal Fallback，不是 Primary Integration**：直接让模型 `bash al ...` 会丢失 structured schema、tool metadata、capability discovery、permission semantics、typed result、streaming、cancellation、lifecycle。

---

## 十六、CLI：扩展已有的 `al`，不新建

v1 建议新建一个 `aleph` 互操作 CLI。**Aleph 已有 `al` CLI**（已合并本地 main）。本轮是在它上面扩展，而不是再造一个：

```
al capabilities     列出（按 scope / principal 过滤）
al exec <capability>
al sessions / al agents
al mcp / al rpc
--json / --jsonl / --stream / --session / --agent
```

具体子命令以清点出的现有 `al` 命令树为准；CLI 只做投影与 I/O（R4）。

---

## 十七、五层架构

```
Layer 5 — Host Integration      dsh Plugin · Pi Extension · CLI · Desktop UI · IDE
Layer 4 — Protocol（出口）       MCP server · ACP agent · JSON-RPC / NDJSON · HTTP / WebSocket
Layer 3 — Capability Runtime    Registry · Scope · Lifecycle · Effects · Events · Policy · Schema
Layer 2 — Agent Runtime         Agent Loop(harness) · Model Router · Context · Memory · Sub-agent · Scheduler
Layer 1 — Execution Substrate   Browser · FS · Shell · Process · Network · GUI · LLM · Storage · Sandbox
                                · ACP client（外部 agent）· MCP client（外部 server）   ← 入口侧，见 §12.3
```

同一个协议可以同时出现在 Layer 4（出口）和 Layer 1（入口）；两处共用同一份报文类型（§12.3 规则 1）。

⚠️ **层与依赖方向要一致**：v1 图中 Kernel 在 Agent Runtime 之上，而 §6.1 又说 Agent Loop 经 Kernel 调用能力。spec 必须画清**编译期依赖方向**（谁 `use` 谁），并与 CLAUDE.md P1 的单向依赖 `Interface → Core → Domain` 对齐。

编译期约束：Aleph Core **不依赖** dsh、Pi 或任何具体 MCP 实现；它们都在 adapter 层。

---

## 十八、原则的最终表述

> **Everything externally composable is a Capability.**
> 一切需要被 Agent、Runtime 或其他系统**动态发现、调用、替换、订阅或管理**的能力，都是 Capability。

`AgentLoop implementation` 不是 Capability；LLM provider、Browser、Shell、Memory、Tool、Task、Sub-agent、MCP connection 可以是。

**判定问句**（取代列举）：*这个东西是否有一个运行时之外的消费者需要发现、调用、替换、订阅或管理它？* 否 ⇒ 它是实现细节，不进 Capability 模型。

---

## 十九、优先级（按 Aleph 现状重排）

| 优先级 | 事项 | 说明 |
|---|---|---|
| **P0-a** | §十四 清点 | 决定范围；不做它，后面都是猜 |
| **P0-b** | Scope / Effect 推广 | 从 `plugin-scope-round` 的实现泛化到 Session / Agent；同轮删掉被取代的清理逻辑 |
| **P0-c** | 单一 descriptor + 多面投影 | 先选一个多面动词做样板，证明 §12.1 的结构性消除（候选之一：ACP harness 的四个面，§12.3） |
| **P0-d** | Registry 收敛 | 按清点结果逐个合并现有注册表，每合并一个删一个 |
| P1 | MCP 面 / `al` CLI 改为从 descriptor 投影 | 两者已存在，改为投影而非新建 |
| P1 | Native dsh Adapter · Native Pi Extension | 两个并列方向，不是 dsh >>> Pi |
| P2 | ACP agent 侧 / Agent 级互操作 | Aleph 作为其他 Agent 的子 Agent / peer；**复用 `src/acp/protocol.rs`，审批语义与 `incoming.rs` 共用推导**（§12.3） |
| P3 | 更多 Host Adapters | Claude Code / Codex / IDE / Desktop / Web |

---

## 二十、总纲图

```
                         ┌──────────────────────┐
                         │        HOSTS         │
                         │ dsh │ Pi │ Codex     │
                         │ Claude │ IDE │ CLI   │
                         └──────────┬───────────┘
                                    │
                         Host / Protocol Adapters
                  （每个出口都从同一份 descriptor 投影）
                                    │
                    ┌───────────────▼───────────────┐
                    │     ALEPH CAPABILITY KERNEL    │
                    │  （由现有注册表收敛而成）       │
                    │ Registry · Scope · Lifecycle   │
                    │ Reversible Effects · Events    │
                    │ Schema · Policy · Cancellation │
                    └───────────────┬───────────────┘
                                    │
                    ┌───────────────▼───────────────┐
                    │         AGENT RUNTIME          │
                    │ Agent Loop（R10 锁定）          │
                    │ Context · Memory · Model Router│
                    │ Sub-Agent · Task Scheduler     │
                    └───────────────┬───────────────┘
             ┌──────────────────────┼──────────────────────┐
             ▼                      ▼                      ▼
          Browser               Execution              Cognition
     Obscura / Chromium      Shell / FS / Process     LLM / Memory
     (经 aleph-cdp)           Sandbox / Network        Retrieval
```

```
dsh 不需要成为 Aleph · Pi 不需要成为 Aleph · Aleph 也不需要成为 dsh 或 Pi
```

---

## 最后：压缩成四句话

1. **dsh 最值得学的**是 Scope + Reversible Effects + 依赖声明——而 Aleph 已经在插件维度做了第一块，本轮是推广，不是从零开始。
2. **Pi 最值得学的**是 Minimal Core + Extreme Extensibility + CLI / RPC / SDK；"Bash-only"不是对 Pi 的准确描述。
3. **dsh 和 Pi 是 Aleph 的 Host，不是 Foundation**；Aleph 的 Foundation 是自己的 Capability Kernel。
4. **Kernel 的价值不在那张图，而在"一份描述、多个出口、所有权可撤销"让几类反复付账的缺陷从结构上写不出来**；衡量它的是删掉了多少旧东西，不是新建了多少新东西。
