# `/doctor` 检测命令 + 「按 f 维修」提醒 — 设计文档

- **日期**: 2026-06-19
- **状态**: 已批准（待转 writing-plans）
- **作者**: brainstorming 会话（rootazero）

---

## 1. 背景与问题 (Context)

期望的用户体验是一个两步闭环：

1. 用户运行 `/doctor` → Aleph 只读检测自身健康，汇报发现的问题；
2. 报告**结尾提醒用户「输入 f 开始维修」**；
3. 用户按 `f` → Aleph 走自我修复流。

代码现状与该描述有出入：

- **修复侧已完整**：webchat 面板裸 `f` 热键 → `chat.request_repair()` → bump `repair_pulse` → composer 监听并注入 `DOCTOR_REPAIR_PROMPT`（"检测 + 机械修复 + 复检 + 报告"）→ 走正常 LLM 发送管线。
  - `interfaces/webchat/src/state/hotkey.rs:119-134`
  - `interfaces/webchat/src/views/chat/composer/mod.rs:269-288`（`DOCTOR_REPAIR_PROMPT` 常量在同文件 :43）
- **检测侧缺口**：
  - **全仓库没有 `/doctor` slash 命令注册**。
  - 检测目前只能经 ① LLM 在用户询问时自调 `doctor(fix=false)` 工具，② CLI `aleph doctor`。
  - **「检测后提醒按 f」这一步不存在**。

## 2. 关键机制认知 (How the code actually works)

- **slash 命令 = 工具，走 L0 fast-path 直接执行、绕过 LLM**：
  `src/gateway/execution_engine/slash_command.rs:43` `try_resolve_slash_command` 把 `/cmd args` 解析为 `{type:"direct_tool", tool_id, args}`，`:88` `execute_slash_command_fast_path` → `tool_registry.execute_tool(...)` 直接执行并流式返回，**中间没有 LLM**。
- `doctor` 工具**已存在**：`src/builtin_tools/doctor.rs`，`DoctorArgs { fix: bool }`（默认 false = 只读），背后是 `DiagnosticEngine`。
- 因此若按现有 slash 机制把 `/doctor` 注册为工具型命令，它会**确定性直跑 `doctor(fix=false)`**——但这条路没有 LLM，**「结尾自然语言提醒按 f」无处安放**。
- 反观 `f` 流：**不是** fast-path，而是"客户端注入 prompt → 走正常 LLM 回路"。

**结论（已与用户确认）**：`/doctor` 采用「走 AI 回路」模型，**完全镜像 `f` 流的机制**，而非 fast-path 直跑工具。

## 3. 红线约束 (Architectural redlines)

- **R1 / R4（大脑四肢分离 / Interface 纯 I/O）**：`f` 是 **webchat 面板专属热键**（CLI、Telegram 无 `f`）。"输入 f 开始维修"这句提示**只在 webchat 面板成立**，**不得**塞进 Core 的 `doctor` 工具输出（否则泄漏给 Telegram/CLI 等无 `f` 通道）。
- **R7 / R9（LLM 主权 / 智慧在 prompt）**：检测推理与提醒措辞**全部交给 LLM**，由 prompt 引导，不用确定性代码替代。
- **R10（薄 Harness / 笨循环）**：**不碰 `src/harness/`**。改动落在 thinker prompt 层 + webchat interface 层。
- **概念完整性（Brooks）**：`/doctor`（检测）与 `f`（修复）走同一机制（客户端注入 prompt → LLM 回路），对称一致。

## 4. 目标与非目标 (Goals / Non-goals)

**目标**

- 注册可发现的 `/doctor` slash 命令（输入 `/` 时可见）。
- `/doctor` 触发**只读**健康检测（绝不修改任何东西）。
- 检测发现**未解决问题**时，LLM 报告结尾自然语言提醒「输入 f 开始维修」。
- `f` 修复流保持**完全不变**。

**非目标**

- 不改 `f` 修复流的行为。
- 不改 CLI `aleph doctor`。
- `/doctor` 不接受参数（v1 忽略一切参数，始终跑全量只读检测）。
- 不在 Telegram/CLI 等非面板通道暴露 `f` 提醒。

## 5. 架构与数据流 (Architecture & data flow)

```
用户输入 /doctor
   ↓ (composer send 拦截：换成检测 prompt — 同 f 注入 DOCTOR_REPAIR_PROMPT 手法)
DOCTOR_DETECT_PROMPT  →  ChatApi::send  →  正常 LLM 回路
   ↓
LLM 调 doctor(fix=false) 只读检测  →  读 findings  →  写自然语言健康报告
   ↓ (若 WebRich 面板 且 有未解决问题)
报告结尾："…输入 f 开始维修"   ← 由 DoctorRepairHintLayer 引导，LLM 措辞
   ↓
用户按 f  →  现有 repair_pulse → DOCTOR_REPAIR_PROMPT 流（不变）
```

## 6. 改动清单 (Components / file-by-file)

### 6.1 `DOCTOR_DETECT_PROMPT` 常量

- **文件**: `interfaces/webchat/src/views/chat/composer/mod.rs`（紧邻 `DOCTOR_REPAIR_PROMPT` :43）
- **内容（中文，与 `DOCTOR_REPAIR_PROMPT` 风格一致）**：
  > "运行 doctor 工具只读诊断系统健康状况（fix=false，不要修复任何东西）。如实汇报发现的问题及其严重度。"

### 6.2 composer 发送拦截

- **文件**: `interfaces/webchat/src/views/chat/composer/mod.rs`（`send_message` :140、`enqueue_message` :226）
- **纯函数** `expand_doctor_command(text: &str) -> Option<String>`：
  - trim 后忽略大小写等于 `/doctor` → 返回 `DOCTOR_DETECT_PROMPT.to_string()`；
  - 否则 → `None`。
  - 纯函数，宿主可单测（参照 `team_history_item_to_message` 模式）。
- 在 `send_message`/`enqueue_message` 取到 `text` 后、注入守卫前调用；命中则把 `text` 替换为检测 prompt。
- **效果**：字面 `/doctor` **不会**到达服务端触发 fast-path，而是以自然语言检测 prompt 进入 LLM 回路。

### 6.3 调色板可发现性（决策 B：客户端静态条目）

- **文件**: `interfaces/webchat/src/views/chat/composer/mod.rs`（`fetch_commands` :337 之后 / `all_commands` :68）+ `palette.rs`（`CommandInfo`）。
- fetch 服务端 `commands.list` 之后，**追加**一个静态 `CommandInfo { key:"doctor", description:"只读检测系统健康，发现问题后可按 f 维修", is_namespace:false, ... }` 到 `all_commands`。
- 去重保护：若服务端目录未来也暴露 `doctor`，避免重复条目。
- 整条 `/doctor`→检测→f 逻辑**聚合在面板层**（与 `f` 热键同为面板本地，高内聚 P2）；**不走服务端工具目录注册**，规避 fast-path 与拦截的先后顺序问题。

### 6.4 `DoctorRepairHintLayer`（新 prompt 层）

- **文件**: `src/thinker/layers/doctor_repair_hint.rs`（**镜像 `voice_mode.rs`**）。
- **gate**: `input.context.is_some_and(|ctx| ctx.environment_contract.paradigm == InteractionParadigm::WebRich)`。
- **注入文案（英文）**：
  > "After running the `doctor` tool in read-only mode (`fix=false`) and finding unresolved problems, end your reply by reminding the user they can press the `f` key to start automatic repair."
- **元数据**：`priority = 1120`（紧随 `SpecialActionsLayer@1100`，落在 finishing/guidance 档区，未占用）；`stability = Dynamic`（参 voice_mode）；`paths()` = `[Soul, Context, Cached]`（生产主路径含 `Cached`，参 voice_mode）；`supports_mode` = `Full | Compact`（镜像 voice_mode — 提醒在压缩态也应保留）。
- **注册三处同步**：
  1. `src/thinker/layers/mod.rs`：`mod doctor_repair_hint;` + `pub use doctor_repair_hint::DoctorRepairHintLayer;`
  2. `src/thinker/prompt_pipeline.rs`：import 列表 + `default_layers()` 加 `Box::new(DoctorRepairHintLayer)` + priority 文档注释。
  3. `src/thinker/prompt_pipeline.rs` 层计数测试 `test_default_layers_count`：`42 → 43`（附带日期注释）。

## 7. 决策 (Decisions)

- **决策 A — 用户气泡显示什么**：**镜像 `f` 流**——气泡显示展开后的检测 prompt 全文（最简单、最对称；与现有 `f` 行为一致）。
- **决策 B — 调色板注册方式**：**客户端静态条目**（面板本地、最聚合）。

## 8. 边界与错误处理 (Edge cases)

- **无问题** → LLM 报告健康，**不**提 `f`（层文案限定 "finding unresolved problems"，由 LLM 判断 — R9）。
- **检测失败** → 走现有工具/回路错误路径，无新增处理。
- **`f` 在非 webchat 不存在** → `DoctorRepairHintLayer` 为 WebRich-gated，即使 doctor 在别处运行也绝不冒出 "press f"。
- **`/doctor` 带参数** → `expand_doctor_command` **仅匹配纯 `/doctor` 单 token**（trim + 忽略大小写）；`/doctor foo` 不匹配，原样下沉（当普通文本/命令处理）。v1 不支持参数，保持最简。

## 9. 测试 (Testing)

- **`DoctorRepairHintLayer`**（镜像 voice_mode 三测）：
  - WebRich paradigm → 注入且含 `f`；
  - 非 WebRich（如 Messaging/CLI/Background）→ 跳过（输出空）；
  - 无 context → 跳过。
- **composer `expand_doctor_command`**（纯函数宿主单测）：
  - `/doctor` ✓、`/Doctor` ✓（忽略大小写）、`  /doctor  ` ✓（trim）；
  - `/doctorx` ✗、`/doc` ✗、普通文本 ✗、空串 ✗。
- **调色板**：输入 `/` 时 `/doctor` 出现在条目中。

## 10. 红线自检 (Redline compliance)

| 红线 | 是否满足 | 依据 |
|------|----------|------|
| R1 / R4 | ✓ | `f` 提示仅面板（WebRich-gated），Core 工具输出不含 `f` |
| R7 / R9 | ✓ | 检测推理 + 提醒措辞全交 LLM，由 prompt 引导 |
| R10 | ✓ | 不碰 `src/harness/`，只动 thinker 层 + webchat interface |
| 概念完整性 | ✓ | `/doctor` 与 `f` 同机制（客户端注入 prompt → LLM 回路） |

## 11. 受影响文件汇总

| 文件 | 改动 |
|------|------|
| `interfaces/webchat/src/views/chat/composer/mod.rs` | +`DOCTOR_DETECT_PROMPT` 常量、+`expand_doctor_command` 纯函数、send/enqueue 拦截、调色板静态条目 |
| `interfaces/webchat/src/views/chat/composer/palette.rs` | （按需）静态条目构造/去重支持 |
| `src/thinker/layers/doctor_repair_hint.rs` | **新增** 层（镜像 voice_mode） |
| `src/thinker/layers/mod.rs` | +mod + pub use |
| `src/thinker/prompt_pipeline.rs` | +注册 + priority 注释 + 层计数测试 42→43 |
