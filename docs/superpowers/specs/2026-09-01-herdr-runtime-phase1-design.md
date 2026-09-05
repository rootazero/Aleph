# herdr 运行时移植 · 第 1 期设计

> **状态（2026-09-04 加，正文一字未改）**：本期交付的 agent 面板**在生产上从未识别过任何一个 agent**——
> 识别源是 spawn 时的 `$SHELL` 标签，而 agent 是在 shell 里交互式启动的。第 2 期换成前台进程探测并在真机上
> 证明了它。**下面的正文刻意保持原样**：它是「第 1 期当时相信什么」的记录，改写它就毁掉了那个缺陷存在过的证据
> （判据 §1：同一事实的两份表述）。
> 现状与全文 → [FEATURE_LOCATOR §6.12](../../reference/FEATURE_LOCATOR.md) ·
> [TERMINAL_RUNTIME.md](../../reference/TERMINAL_RUNTIME.md) ·
> 第 2 期 spec → [2026-09-04-terminal-round2-design.md](2026-09-04-terminal-round2-design.md)。

> **背景、逐层对账、两条裁定的账本、明确不搬清单** → [评估页面](https://claude.ai/code/artifact/45b5b9ba-dfba-4285-a7db-fccc4b43d069)
> **本文件不复述那些论证**（同一事实的两份表述＝判据 §1）。这里只写第 1 期的实施契约。
>
> 前置裁定已落库：`CLAUDE.md:23`（R3 例外·运行时定位）· `CLAUDE.md:69`（禁用清单·第二个 VT 实现）
> 项目状态记忆：`project-herdr-runtime-port`

---

## 1. 范围

**本期定义的"完成"**：左栏能显示别人 agent 的 `working / blocked / idle / unknown`，TUI 与 Panel 两端显示同一份判断，LLM 能读到同一份状态。

**不做**（第 2 期，其中 VT 扩容需先有 0-A 缺口清单）：VT 扩容 · 多会话 tab/split · tiling 布局 · workspace/worktree 模型 · PTY 写入 · manifest 远端热更新。

**本期不依赖 0-A。** 下面每一项的输入都已在现有代码中确认存在或确认缺失，无占位符。

---

## 2. 本期新增的两条裁定（2026-09-01）

| 裁定 | 选择 | 代价 |
|---|---|---|
| 左栏形态 | **并存·同列分段**：`mode_sidebar` 图标列不动；chat 左栏那一列拆成两段——上「运行中的 agent」、下「会话」，可拖分割比 | 两个概念挤一列，窄屏要做取舍 |
| PTY 授权 | **第 1 期只读**：工具面只给 list / read / status，写入仍然只有人能做 | 砍掉「agent 自己开终端跑长任务」，推到第 2 期 |

第二条的理由是它避开了一个真难题：在 PTY 输入字节流上识别命令边界（分次输入、heredoc、交互式提示都能绕过）。做不好就是一次授权洗白。只读版本零新增授权面，而核心价值——**LLM 能看见别人 agent 卡在哪**——已经解锁。

---

## 3. 数据流

```
PTY 子进程
   │  字节
   ▼
gateway/pty/screen/          ← 现有，本期不改语义
   ├─ grid.row_text(row)     ← 已存在，detect 的文本入口
   └─ state.title            ← 已存在（OSC 0/2，含跨读分片处理）
   │
   ├──► [4.2 采样] ──► agent-detect ──► RuntimeAgentEntry 表
   │                    (纯函数)              │
   │                                          ├──► [4.3] runtime.agents.list / .changed
   │                                          │         │
   │                                          │         ├──► [4.4] shared/ui_logic::agent_panel
   │                                          │         │            ├──► [4.5] TUI ratatui 绘制
   │                                          │         │            └──► [4.5] Panel Leptos 绘制
   │                                          │         │
   │                                          └──► [4.6] terminal 工具面（只读）
   │
   └──► pty.screen 差分帧 ──► 现有终端渲染（本期不动）
```

采样挂在差分帧上，**不另起 timer**——两个时钟就是两个顺序（判据 §12）。

---

## 4. 组件契约

### 4.1 `crates/agent-detect/` — Apache-2.0 隔离区

**独立 crate 而不是 `src/` 下的目录**，理由是许可证边界要**机器可读**：Cargo.toml 的 `license = "Apache-2.0"` 字段能说话，目录注释不能。这也让「哪些文件是抄来的」永远只有一个答案。

- **移植范围**：herdr `src/detect/{mod,manifest,manifest_update}.rs` + `src/pane/agent_detection.rs`，实测 4,725 行
- 保留原 Apache-2.0 许可头；crate 根放 `NOTICE`
- **对外只暴露纯函数**：`detect(DetectionInput<'_>) -> AgentDetection`
- 依赖只有 `regex` + `serde`。**不依赖 alephcore，不依赖 tokio**——它必须能被 `interfaces/tui` 那种禁止依赖 core 的 crate 直接用
- manifest 来源本期**只接 `Bundled`**（编译期内嵌）。`Remote` 热更新引入网络与信任面，不在本期背

herdr 的类型原样保留，不重命名：`AgentState { Idle, Working, Blocked, Unknown }`、`AgentDetection { state, skip_state_update, visible_idle, visible_blocker, visible_working }`。改名会让上游的修复无法对照搬运。

### 4.2 服务端采样 — 旁挂在 `src/gateway/pty/screen/`

`DetectionInput` 有三个字段，Aleph 这一侧的供给情况**已核实**：

| 字段 | 现状 | 本期 |
|---|---|---|
| `screen` | `grid.row_text(row)` 已存在 | ✅ 拼**当前可视区全部行**（`rows` 高度），不含 scrollback |
| `osc_title` | `screen.state.title` 已存在；`osc_dispatch` 注释明写 *OSC 0 = icon + title, OSC 2 = title*，且有「分片到达不截断」的测试 | ✅ 直接取 |
| `osc_progress` | `osc_dispatch` **不认 OSC 9;4** | ⚠️ **传空串** |

**`osc_progress` 传空串是一次有意的削弱，不是遗漏。** herdr 的引擎对空串的行为等同 pre-OSC 版本（其 `DetectionInput` 文档明写这一点），所以功能正确、只是更弱。

> ⚠️ **必须在代码里写下这个事实并配一条断言**，否则六个月后会有人以为 progress 已经接过了。空串只有资格说「我不知道」，不许被读成「没有进度」（判据 §8）。OSC 9;4 的接入登记进 0-A 缺口清单。

**为什么是可视区而不是「尾部 N 行」**：manifest 匹配的是 agent 当前画在屏上的 chrome，不是历史输出。行数从 `grid` 的 `rows` 派生，不引魔数（P6）——窗口一 resize，取的范围自动跟着变（判据 §12：边界与它比较的东西共用同一处派生）。

- 采样跟差分帧走，不另起时钟
- **不进 `src/harness/`**（R10）

### 4.3 wire — `shared/protocol`

```rust
pub struct RuntimeAgentEntry {
    pub session_id: String,
    pub label: String,
    pub cwd: String,
    pub agent: Option<String>,   // None = manifest 不认识它
    pub state: RuntimeAgentState, // idle | working | blocked | unknown
    pub updated_at: i64,
}
```

- RPC：`runtime.agents.list`
- 事件：`runtime.agents.changed`

**`seen`（「人是否已看过这次状态变化」）本期 CUT。** 它在 herdr 里是注意力流的一环，但在这里它需要一个写入动词，而本期没有写入面——留下字段就是一个零写者的槽，正是要被 CUT 的抽象（R10 / 判据 §7「两端完整而中间没线」）。第 2 期若真需要，连同它的写者一起加。

> ⚠️ **判据 §10**：键集放进 `aleph-protocol`，**并用它构造响应**——服务端不许手搓 `json!`。一个只解析自己刚写下的字面量的测试测的是 serde，永远绿。

**授权复用现有闸**：`gateway/handlers/pty.rs` 的模块文档明写 *Operator-only, on BOTH faces*——工具面与 RPC 面都已封好。`runtime.*` 走同一道闸，**不新造授权面**（判据 §9：一个动词有几张脸，判据就要在每张脸上用同一个推导）。

### 4.4 `shared/ui_logic::agent_panel` — R2 的唯一落点

双左栏的合规形状就落在这里。这个 crate **已经**同时被 `interfaces/tui`（`default-features = false`）和 `interfaces/webchat`（`leptos + wasm`）依赖，里面已经住着 `state/composer_queue`、`state/chat_scroll`、`state/composer_dials` 这类跨端派生状态——agent 面板的模型是同一类东西。

承载：注意力排序、分段折叠、分割比。

**本期不分组**——herdr 的分组键是 worktree 父子关系，而 worktree 模型在第 2 期。现在做分组就是给一个还不存在的层级造 UI。

- 在 `default-features = false` 下**必须能编**（TUI 用）
- 在 `leptos` feature 下额外暴露信号（Panel 用）
- **排序规则只写一次**，两端都不许自己再排
- 排序是**单键**：`blocked > working > idle > unknown`。CUT 掉 `seen` 之后不存在第二个键，也就不存在「谁压谁」的歧义。同状态内按 `updated_at` 降序，稳定

### 4.5 左栏渲染 · 两端

形态按裁定：同列两段，上「运行中的 agent」、下「会话」，中间可拖的分割条。

**TUI** — `interfaces/tui/src/tui/widgets/agent_panel.rs`
抄 herdr `ui/sidebar.rs` 的绘制骨架（ratatui 0.29 ↔ 0.30，差一个小版本），**换根到 4.4**。

> ⚠️ 硬约束：`interfaces/tui/Cargo.toml` 明写 **MUST NOT depend on alephcore**。herdr 的 sidebar 读它进程内的 `AppState`——换根是必做的那一步，不是可选的清理。

**Panel** — `interfaces/webchat/src/components/sidebar/agent_panel.rs`
Leptos 重写绘制，消费同一个 4.4。

> ⚠️ Leptos 0.8 的 `window_event_listener` **不注册任何清理**。分割条拖拽若挂全局监听，必须自己持 handle 并在卸载时 drop——否则每次访问泄漏一个。

### 4.6 `terminal` 工具面 — 只读

- `terminal.list` — 列出会话
- `terminal.read` — 读**当前可视屏幕**（与 4.2 的 detect 输入同源），不含 scrollback
- `terminal.status` — 读 detect 判断

**没有写入动词**（本期裁定）。继承 `pty.*` 的 operator 闸。

> `DESCRIPTION` 里必须写清「只读」——这句话归这个工具所有，不进 system prompt（R9 第二把尺）。不写，模型会反复尝试发命令。

---

## 5. 错误与降级

| 情况 | 正确行为 | 错误行为（判据 §8） |
|---|---|---|
| detect 返回 `Unknown` | 显示「我无法判断」 | 渲染成 idle |
| manifest 不认识这个 agent | `agent: None`，状态 `Unknown` | 猜一个最像的 |
| PTY 会话消失 | 条目消失 | 留幽灵条目 |
| `runtime.agents.list` 被拒（非 operator） | 显示 `admin_refusal` | 显示空列表——「被拒」不许读作「没有」 |
| `osc_title` 尚未到达 | 空串，走 pre-OSC 路径 | 当成「标题是空的」 |

---

## 6. 测试

- **agent-detect**：herdr 的测试一起搬——它们是 Apache-2.0 交付物的一部分，也是这批 regex 唯一的证伪装置
- **一条证伪过的守卫**：剪断 4.2 的 `osc_title` 接线，`runtime.agents.list` 返回的 title 必须变空。不会变红的守卫不是守卫（判据 §3）
- **wire 契约**：用 `aleph-protocol` 的类型**构造**响应再解析，不是只解析字面量（判据 §10）
- **ui_logic**：注意力排序的属性测试（`blocked` 恒在 `working` 前；同状态内 `updated_at` 降序且稳定）
- **双端一致性**：同一份 `Vec<RuntimeAgentEntry>` 喂进 `agent_panel`，TUI 与 Panel 的可见条目集合必须相等——这条是 R2 的自动化表达

## 7. 验证集增量

在 CLAUDE.md 的六条之外：

```
cargo test -p agent-detect
cargo test -p aleph-tui                       # 改了 interfaces/tui 的同一笔里
cargo test -p aleph-panel --lib               # 改了 interfaces/webchat 就跑，哪怕不是你改的
```

## 8. 明确不做

VT 扩容（等 0-A）· 多会话 tab/split · tiling 布局 · workspace/worktree 模型 · PTY 写入动词 · manifest 远端热更新 · OSC 9;4 progress 接入（登记进 0-A 清单）。

## 9. 风险

**最大** — detect 的 manifest 是对 herdr 那批 agent **某个版本**输出 chrome 的快照。agent 改了界面，识别就静默失效，而失效的样子是「一直显示 idle」。
缓解：`Unknown` 在 UI 上显式可见（不伪装成 idle）＋ manifest 版本号显示在面板上，让「它多久没更新了」是可见的。

**次大** — 只读工具面可能不够用。这个第 2 期才知道，本期不预先设计写入路径（预留的接口没有消费者就是要被 CUT 的抽象，R10）。

**未知** — 无。本期每一项的输入都已在现有代码中核实。

---

## 10. 自审记录（2026-09-01）

写完后按 brainstorming 的 spec self-review 跑了一遍，改掉四处：

| 类型 | 发现 | 处置 |
|---|---|---|
| 占位符 | `screen` 取「尾部 N 行」，N 未定义 | 改为从 `grid.rows` 派生的可视区，无魔数 |
| 内部矛盾 | §4.4 承载「分组」，§8 却把 worktree 模型排在第 2 期 | 本期不分组，两处对齐 |
| 悬空的写者 | `seen` 字段没有任何写者（本期无写入面） | CUT，连同「未看过优先」的排序键 |
| 歧义 | 两个排序键谁压谁未定义 | CUT `seen` 后成单键；同状态内按 `updated_at` 降序 |
| **自审的漏网** | 改完 §4.3/§4.4 后，§6 的测试项仍写着「未看过恒在看过前」——`seen` 已 CUT | 补掉。**一次自审不足以覆盖它自己造成的改动**，扫第二遍才抓到 |

**过程偏差（记在这里，不藏）**：brainstorming 的步骤 4–5（提 2–3 个方案、分节逐段批准）本轮被压缩——两个真分叉（左栏形态、PTY 授权）已由用户当场裁定，其余没有需要选的。这是为节省额度做的取舍，不是流程遗漏。
