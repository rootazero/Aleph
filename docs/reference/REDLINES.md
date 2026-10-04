# Architecture Redlines & Tech Stack Constraints

> **Tier 1 只留一句话版；本文是完整版（带例外条款与推导锚点）。** 代码话术与触发器见 [FEATURE_LOCATOR.md](docs/reference/FEATURE_LOCATOR.md) 附录 E。

---

## 🛑 Architecture Redlines (R1–R10)

> 最高优先级约束，违反的代码不得合入。这里只留**禁令**，例外与推导见指针。

| # | 红线 | 禁令 / 原则 |
|---|------|------------|
| **R1** | 大脑与四肢绝对分离 | 严禁在 `src` 中直接调用平台系统 API（AppKit / Vision / CoreGraphics / windows-rs）；核心只定义能力契约 (Trait)，物理实现由原生 Bridge (Swift / 其他) 经 IPC 提供。**例外·进程隔离内核**：restricted-token / job-object / AppContainer / 完整性级别 / SID·ACL 与本地 PID 探测**必须由 spawn 子进程的父进程就地发起**，无法经 IPC 桥委托 ⇒ `src/sandbox/*` 与 `builtin_tools/desktop/session_lock.rs` 的平台 FFI（`cfg(windows)` 门控）是**立意之外的合法开口**，非违规——R1 针对的是桌面 UI / 屏幕 / Vision **四肢** → [SANDBOX.md](docs/reference/SANDBOX.md) |
| **R2** | UI 逻辑唯一源 | 严禁在原生 Bridge 中实现有业务逻辑的设置页 / 表单 / 列表；复杂业务 UI 一律在 Leptos (WASM) Panel，Bridge 只做系统 API 调用与桥接 |
| **R3** | 核心轻量化 | 严禁为单一非核心功能往 core 引入沉重三方库；优先实现为 Skill (Python/Bash) 或 MCP Server。**内核只调度，不搬砖**。**例外·运行时定位（2026-09-01 用户裁定）**：「跑别人 agent 的运行时」是 Aleph 的**核心定位**——为它引入的重量不属"单一非核心功能"，此条不挡路；但仍须逐项答出**为什么这一块不能是 Skill / MCP**，且不得据此绕开下方禁用清单 |
| **R4** | Interface 层禁止业务逻辑 | Channel / Bot / CLI / Panel 不做数据持久化、记忆检索或任务规划——纯 I/O：输入转 JSON-RPC 发给 Server，响应渲染给用户 |
| **R5** | AI 主动到达 | 通过用户**已有的**工作通道主动送达（多端推送 / 内联建议 / 订阅式 Daemon 触发）；不抢焦点、不弹模态，但不因此砍掉必要的交互入口 |
| **R6** | 一核多端 | Aleph 是常驻后台服务，UI 不是必需品；Rust Core 是唯一大脑，多端通道只负责 I/O 与渲染，不参与业务推理 |
| **R7** | LLM 主权 | 严禁用确定性代码替代 LLM 擅长的推理判断（意图识别 / 任务评估 / 路由决策 / 内容分类）。对每个模块问：**这是在赋能 LLM，还是越俎代庖？** 赋能层（Gateway/Memory/Daemon/Soul/Provider/Tool/MCP/压缩/安全硬过滤）保留；意图规则引擎、POE 验证管线、多层 Tool Filter、Context 多层合并、Dispatcher 意图分析一律禁止 |
| **R8** | 工具即一切 | Aleph 自身**所有可配置操作**都暴露为工具，让 LLM 用自然语言完成配置（agent / provider / channel / skill / MCP / daemon 规则）。核心循环：`用户自然语言 → LLM 理解意图 → LLM 选择工具 → 工具执行 → 结果返回 LLM → LLM 回复用户`。**对话即管理面板** |
| **R9** | 智慧在 Prompt 中 | 被移除的中间件的智慧**迁移**到 system prompt，不是丢弃。**但 prune-the-prompt**：模型越强越需要更少方向 / 约束 / 示例，新模型发布后第一件事是**修剪**上下文。加字节前过两把尺——① 这是模型**做不到的运行时事实**，还是我在教强模型怎么思考？② **有没有一个工具拥有这句话**？有 → 写进那个工具的 `DESCRIPTION`。两把尺已建进 `src/thinker/prompt_contract.rs`，量一下用 `aleph-server prompt-size`。⚠️ 第二把尺**有前置条件**（目录条目写字面量会整体遮蔽工具常量——附录 E.3），尺子量不到的地方搬过去等于删掉 → [HARNESS_PHILOSOPHY.md §8](docs/reference/HARNESS_PHILOSOPHY.md) |
| **R10** | 薄 Harness，笨循环 | `src/harness/` 锁 **12 文件**、只承载 Think→Act 轮次调度、循环里有 **5 个"不"**、加代码前必答 **3 问**、任何"零消费者"的抽象立即 CUT（YAGNI 撤回）。**行数红线＝棘轮机制本身**（`src/harness/tests/budget.rs::CEILING`，实测非手算、只减不增、增必答 3 问）——**代码是权威，本文件刻意不复制那个数字**，因为文档抄一份就漂移过一次 → [HARNESS_PHILOSOPHY.md](docs/reference/HARNESS_PHILOSOPHY.md) · [`src/harness/CLAUDE.md`](src/harness/CLAUDE.md) |

> **R10 的渐进式工具披露例外**：「core 工具静态常驻 + 全量目录 + `tool_search` 按需加载 schema」是**不看消息内容的静态分区**、加载决策 100% 由模型发起，与 `src/tools/scoped/` 已有的三道静态 `retain` 同层同性质，**不属**第 2 不所指的"按意图过滤"；它落在工具呈现层，**不进 `src/harness/`**。

---

## 🛠 Tech Stack & Do NOT introduce

**核心栈**: Rust Core (tokio + serde) · 记忆层 SQLite + sqlite-vec · 接口 JSON Schema (schemars) · Panel Leptos/WASM · 桌面壳 Tauri。

**Do NOT introduce unless explicitly requested**（基于 R1/R3/R7 推导，违者不得合入）:

- **为 Aleph 自身代码引入第二个 async runtime**（async-std / smol）—— 一方代码全栈锁定 tokio（Cargo.lock 中的 async-std 是三方传递依赖，不影响此禁令）
- **独立向量数据库 client 进 core**（qdrant / lancedb / milvus 等）—— 记忆层已锁 sqlite + sqlite-vec
- **`src` 中直接依赖平台 API crate**（windows-rs / core-graphics / cocoa / objc / winapi）—— 违 R1，必须走原生 Bridge IPC
- **正则 / 规则引擎做意图识别或路由** —— 违 R7/P8，语义判断交 LLM
- **非 serde 的序列化栈** —— 全栈 serde
- **第二个 VT / 终端模拟器实现**（含移植 herdr 的 `pane/terminal` + `terminal/state`）—— 服务端 VT 的唯一真源是 `src/gateway/pty/screen/`；跑别人 agent 需要的能力（alt-screen / graphics / OSC 进度 / kitty keyboard）一律**扩容它**，不引第二份（2026-09-01 用户裁定；判据 §1「同一事实的两份表述」）
- **第二个 CDP 客户端实现**（`chromiumoxide` / `headless_chrome` / 在 `src/browser/` 里再手写一份）—— 唯一真源是 **`crates/aleph-cdp`**（零 Aleph 依赖的传输层：一条连接、多 session、逐命令超时、断连时把全部 pending 一次性失败）。两个引擎、每一个 `browser_*` 动词、`page_state` 的两个 fetcher 全走它；缺方法就**给它加一个 `methods::` 包装**，不引第二份（2026-09-06；判据 §1）
