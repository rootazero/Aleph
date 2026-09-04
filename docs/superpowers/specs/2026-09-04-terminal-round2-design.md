# 多 agent 终端窗口 · 第二轮设计（herdr 运行时移植 第 2 期）

**日期**: 2026-09-04
**分支**: `worktree-terminal-round2`（基于 main `43a50d30f`）
**参照**: `/Volumes/TBU4/Github/herdr`（0.8.2 · Apache-2.0）
**前置**: [第 1 期 spec](2026-09-01-herdr-runtime-phase1-design.md) · [0-A VT 缺口清单](../plans/2026-09-03-0a-vt-capability-gaps.md) · [Panel 内嵌终端 spec](2026-08-29-panel-embedded-terminal-design.md)
**探索产物**（本轮三份只读清单，未入库，路径见记忆条目）: Aleph 审计 473 行 · herdr 运行时清单 1,287 行 · herdr agent API 清单 240 行

> 本文只写第 2 期的**裁定与契约**。herdr 的机制细节引用清单里的 `path:line`，不复述（判据 §1）。
> ⚠️ **过程说明**：本轮为无人值守会话，用户协议明确要求「扫描 → 计划 → 实施 → 提交」一气完成。brainstorming 的逐段批准门被压缩为「spec 落盘＝方案已给」；所有需要用户裁定的分叉都集中在 §7 **DECIDE** 段，本轮**不实施**那些项。

---

## 0. 一句话

第 1 期交付的 agent 面板**在生产上从未识别过任何 agent**（`src/gateway/runtime/mod.rs:99-131` 自陈：只用 spawn 时的 `$SHELL` 标签去识别），21 份 manifest、规则引擎、idle-hold、OSC 9;4 全是正确但不可达的代码。第 2 期的核心是**接通这一条线**（前台进程探测 → 识别），然后把 Panel 从「只能看最老的一个终端」变成「多 tab + 从 agent 面板一键跳到卡住的那个」，同时把 0-A 里会污染 `visible_text()` 的 VT 缺口补掉。

---

## 1. 对比分析摘要（Gap Analysis）

| 维度 | herdr（参照） | Aleph 现状（本轮审计） | 本轮裁定 |
|---|---|---|---|
| **agent 识别** | 前台进程组 (`tcgetpgrp`/`proc_pidinfo`) + argv/cmdline 归一化 (`src/detect/mod.rs:243` `identify_agent_in_job`)，探测有闸（`should_probe_foreground_job`）、6 次 miss 才遗忘 | **D1**：`identify_agent(session.shell)`，永远 `"zsh"` ⇒ 永远 `Unknown` | **做**（W-A）：`portable-pty` 的 `process_group_leader()` + `sysinfo` 单 pid 刷新，不引平台 crate，不违 R1 |
| **状态仲裁** | 四源：屏幕兜底 / hook 权威 / 进程事实做闸 / metadata；五种去抖常量 | 单源：屏幕；idle-hold 700ms 已移植 | 屏幕+进程事实（本轮）；hook 源列为第 3 期（§8） |
| **静默的 agent** | 无时间衰减；但 `seen`/`Done` + 注意力排序让它可见 | **D4**：Working 后 PTY 沉默 ⇒ 永远 Working，`updated_at` 无人渲染 | **做**（W-A）：服务端 `quiet_since`（30s 无帧）作为事实发布，**不**用时间伪造 idle |
| **live cwd** | OSC 7/9;9/1337 + PID 兜底，`cwd` 与 `foreground_cwd` 分开 | spawn 目录，永不更新；OSC 7 到达即丢 | **做**（W-A/W-B）：OSC 7 ＞ 前台进程 cwd ＞ spawn cwd |
| **多窗口模型** | workspace → tab → BSP 树；pane 只是指向 terminal 的布局槽 | **D2**：一个 `TerminalView`，attach 到 `pty.list` 第一个未关闭会话 | **做 tab**（W-C）；BSP 分屏推到第 3 期 |
| **面板 ↔ 终端连线** | 侧栏点击即聚焦；toast 可点击跳转 | **D3**：agent 行是纯 `<div>`，两端都在、中间没线 | **做**（W-C） |
| **VT 覆盖** | libghostty 全保真 | 0-A Tier A：RIS/DECSTR、DECAWM、IND/NEL、滚动区+DECOM、REP、47/1047 缺；OSC 标题 `;` 截断（**D6**） | **做**（W-B），全部在 `screen/` 内扩容 |
| **粘贴** | bracketed paste 按目标终端实时模式包裹 | **D11**：mac 无粘贴；Win/Linux Ctrl-V 发 0x16 且抑制浏览器粘贴 | **做**（W-B 跟踪 `?2004` + W-C `on:paste`） |
| **wire 契约** | 全部 schemars 类型 | **D9**：`pty.list` 三处手搓键集，无共享类型 | **做**（W-0） |
| **LLM 工具面** | `agent {get,read,wait,explain,prompt,send-keys,start}` | `terminal{list,read,status}` 只读 | **做只读增量** `wait`/`explain`（W-D）；写动词 → §7 DECIDE |
| **协议层授权** | 无；0600 socket，同用户即全权 | 双面 operator 闸 + 单源 `owner_admits`；但 **D7** 零身份路径读到所有人的屏幕 | **做**（W-D）：零身份臂只放行零归属会话 |
| **Panel 失败态** | — | **D8**：Panel 把「被拒」与「不可用」并成一个红框，TUI 分开 | **做**（W-C） |
| **前端重复** | 单一 `AgentStatus` 投影 | **S6**：四态字形在两端各抄一份，无交叉测试 | **做**（W-C）：搬进 `shared/ui_logic` |
| **成本纪律** | 隐藏 pane 只解析不呈现；探测有闸；计数式架构测试 | 每帧每会话 `visible_text()` 先分配再丢弃（**S3**） | **做**（W-A）：识别前置、探测限频、计数守卫 |
| **持久化 / handoff** | 快照只存 cwd/argv/session-ref；fd 经 SCM_RIGHTS 交接 | 无（重启即丢） | **不做**（第 3 期，§8） |
| **agent hooks** | 20 个集成资产，走同一 socket；6 个全生命周期，其余只报 session id | 无 | **不做**（第 3 期，§8） |

---

## 2. 本轮裁定（2026-09-04）

| # | 裁定 | 理由 / 代价 |
|---|---|---|
| R2-1 | 识别源 = **前台进程事实**，不是 spawn 参数 | 用户是交互式启动 agent 的（先开 shell 再敲 `claude`），`command` 只修没人用的那条路 |
| R2-2 | 进程探测**只用 `portable-pty` + `sysinfo`**，不加平台 crate | 两者已是直接依赖；R1 针对的是桌面四肢，进程事实与 `utils/process_alive.rs` 同类 |
| R2-3 | 沉默不等于 idle：发布 `quiet_since`，**不**做时间衰减 | 时间衰减是在伪造证据（判据 §8）；herdr 也不做。让「安静了 5 分钟」可见即可 |
| R2-4 | Panel 第 2 期做 **tab 条**，不做 BSP 分屏 | tab + 面板跳转已覆盖「别再找卡住的那个」；分屏是 ~600 行零测试环境的 Leptos，留第 3 期 |
| R2-5 | `terminal` 工具本轮**仍只读**，新增 `wait`/`explain` 两个观察动词 | 写动词＝给 LLM 一个绕过 `[sandbox.command_policy]` 的 shell，是授权架构决定 → §7 |
| R2-6 | 所有 VT 扩容留在 `src/gateway/pty/screen/` | CLAUDE.md 禁用清单；0-A 已定价 |
| R2-7 | 三个工作流并行时**各占一棵 worktree**，文件集不相交 | 记忆 `feedback-one-agent-owns-the-tree`：同一棵树两个 agent 会把对方的变异读成自己的残留 |

---

## 3. 数据流（第 2 期之后）

```
PTY 子进程
   │ 字节
   ▼
pty/session.rs spawn_reader ──► screen/perform.rs (vte)
   │                              ├─ grid  (+滚动区/DECAWM/RIS/REP/47|1047)
   │                              ├─ title (OSC 0/2, 修 `;` 截断)
   │                              ├─ osc_progress (OSC 9;4)          [已有]
   │                              ├─ cwd (OSC 7, file:// 严格 host)   [新]
   │                              ├─ cursor_visible (?25) · bracketed_paste (?2004)  [新, 进 patch]
   │                              └─ hook/put/unhook 显式 no-op       [新]
   ▼
pty/manager.rs start_flush_loop (16ms, 唯一时钟)
   ├─ (a) take_patch ──► pty.screen 帧 ──► Panel views/terminal (tabs + paste + cursor)
   ├─ (b) foreground.rs probe (限频, 见 §4.1) ──► session.foreground 缓存
   └─ (c) runtime::agents().sample(program=foreground∨shell, cwd=osc7∨fg_cwd∨spawn, screen)
            ├─ identify_agent(program) ─► manifest 规则 ─► state
            ├─ quiet_since 翻转 (30s)                      [新]
            └─ changed ⇒ runtime.agents.changed (空载荷, 不变)
                  ├─► TUI/Panel 面板 (字形/排序共用 shared/ui_logic)
                  ├─► Panel 面板行 on:click ─► PanelMode::Terminal + 选中该 session  [新]
                  └─► terminal{status|wait|explain}                                [wait/explain 新]
```

---

## 4. 组件契约

### 4.1 W-A · 识别连线 — `src/gateway/pty/foreground.rs`（新）+ `runtime/`

**探测**（`ForegroundFact { pid, name, argv0, cmdline, cwd, observed_at }`）：
- Unix：`MasterPty::process_group_leader()`（`portable-pty 0.8.1 src/unix.rs:332`，内部 `tcgetpgrp`）得 pgid → `sysinfo` 单 pid 刷新（沿用 `src/utils/process_alive.rs` 的 `with_process` 形状）取 `name / cmd / exe / cwd`。
- 非 Unix：`sysinfo` 全表刷新一次，取 shell pid 的**最深、最新**后代。
- **限频**（herdr `src/pane.rs:776-789` 的闸，简化为三条）：① 屏幕本 tick 有帧且距上次探测 ≥ `PROBE_MIN_INTERVAL_MS`(500)；② 已识别到 agent 且距上次 ≥ `PROBE_RECHECK_MS`(3000)（agent 退出要能被发现）；③ 从不在 screen 锁内探测。
- **消失滞后**：连续 `PROBE_MISSES_TO_FORGET`(6) 次探不到才把 `program` 退回 shell 标签（herdr `AGENT_MISS_CONFIRMATION_ATTEMPTS`）；出现即刻生效。
- 计数守卫（herdr §8.5 形状）：thread-local 计数器数 `sysinfo` 刷新次数，断言 15 个会话 × 100 个 tick 的探测次数 ≤ 上界，并有第二条测试证明计数器能到 1。

**识别**：`agent_detect` 新增 `identify_agent_from_process(name, argv0, cmdline) -> Option<Agent>`，移植 herdr `normalized_process_name` + `process_priority`（`src/detect/mod.rs:243-271`），覆盖 `node /path/claude`（argv 扫描）这一类。Apache-2.0 头保留。

**`RuntimeAgents::sample`**：
- 签名改为接 `program: &str`（探测结果 ∨ shell 标签）与 `cwd`（OSC 7 ∨ 前台 cwd ∨ spawn cwd）；`identify_agent` **移到 `visible_text()` 之前**（S3）。
- idle-hold 加回 `agent_changed` 项（现在 agent 可变了）。
- `changed` 谓词加 `program`、`quiet_since` 的 **None↔Some 翻转**（不是值）。

**wire**（`shared/protocol/src/runtime.rs`）：
```rust
pub struct RuntimeAgentEntry {
    /* 既有字段不变 */
    /// 前台程序名（探测事实）。None = 本平台/本会话探不到，不是"没有程序"。
    pub program: Option<String>,
    /// 连续 QUIET_AFTER_MS 没有帧时 = Some(最后一帧时刻, ms)；有帧则 None。
    /// 只有资格说"安静了多久"，不说"它闲了"。
    pub quiet_since: Option<i64>,
}
```
`cwd` 文档改为「live cwd，来源顺序 OSC 7 › 前台进程 › spawn」。

**守卫**（判据 §3/§4）：
- `a_real_agent_started_after_spawn_is_identified`：真 PTY 起 `sh`，PATH 前置一个名为 `claude` 的脚本，脚本画出 `claude.toml` 里某条 `working` 规则匹配的 chrome；断言 `runtime.agents.list` 的 `agent == Some("claude")` 且 `state == Working`。**这条替代 D10 那条只在函数内证明的守卫**。
- `a_quiet_working_agent_reports_quiet_since_without_becoming_idle`。
- `probe_count_is_bounded_at_fifteen_sessions`（计数式）。

### 4.2 W-B · VT 扩容 — `src/gateway/pty/screen/`

按 0-A 的优先序，全部**只扩 `screen/`**：

| 项 | 序列 | 网格状态 | 备注 |
|---|---|---|---|
| A4 | `ESC c` RIS · `CSI ! p` DECSTR | 清屏/归位/清 SGR/退出 alt/重置滚动区与 DECAWM/清标题 | RIS **不**清 scrollback（写进注释） |
| A3 | `CSI ?7 h/l` DECAWM | `autowrap: bool` | 关掉时 `put` 钉在最后一列，**不得**进入 `cursor_col == cols` 那个"欠一次换行"态 |
| A5 | `ESC D` IND · `ESC E` NEL | — | 复用 `newline()` / `carriage_return()` |
| A2+B5 | `CSI r` DECSTBM · `CSI S/T` · `ESC M` RI · `CSI ?6 h/l` DECOM | `scroll_region: (top, bottom)`，`origin_mode: bool` | `newline/scroll_up/insert_lines/delete_lines/erase_in_display` 五处改读区域；**区域内滚出的行不进 scrollback**；resize/RIS 重置；`goto` 在 DECOM 下加偏移 |
| A6 | `CSI Ps b` REP | `last_printed: Option<char>` | `execute` 与 CSI 派发都清空它 |
| A8 | `?47` `?1047` `?1048` | 复用 `saved`/`saved_cursor` | 47/1047 **保留**进入时的 alt 网格，1049 才新建 |
| C9 | `CSI ?25 h/l` | `cursor_visible: bool` | 进 `PtyScreenPatch.cursor_visible: Option<bool>`（变化才 Some） |
| C5 | `CSI ?2004 h/l` | `bracketed_paste: bool` | 进 `PtyScreenPatch.bracketed_paste: Option<bool>`；W-C 的粘贴据此包裹 |
| B1 | OSC 7 | `cwd: Option<String>` | `file://` 只认空 host 与 `localhost`（herdr `src/pane/osc.rs:629-659`）；百分号解码 |
| D6 | OSC 0/2 | — | `params[1..]` 以 `;` 重连，与 `retain_osc_progress` 同一写法 |
| C1 | `hook/put/unhook` | — | 显式空实现 + 注释 |

结构：`perform.rs` 的 1,191 行测试搬到 `screen/perform/tests.rs`（或 `screen/tests/`），生产代码保持在 500 行以内的模块拆分（`perform/{csi,esc,osc}.rs` 按派发面拆）。`convert.rs` 透传新字段。

**守卫**：每个序列一条「有它/没它」对比测试（同一输入，断言 `visible_text()` 不同）；`vim` 风格的 `CSI 2;23r` + 滚动样例断言首行不动；DECAWM 关时最后一列覆盖而非换行；RIS 后 `visible_text()` 为空且 `title()` 为 None。

### 4.3 W-C · Panel 多终端 — `interfaces/webchat/`

**tab 条**（`views/terminal/tabs.rs`，纯模型 + 组件分离，模型可单测）：
- 数据源 = `pty.list`（W-0 的共享类型）∪ `pty.exit` topic ∪ `runtime.agents.list`（按 `session_id` join 出字形与 `program`）。
- 每个未关闭会话一个 tab；标题 = OSC title（S7 修零调用者）∨ `program` ∨ shell；tab 前缀四态字形。
- 动作：新建（`pty.spawn`，默认 shell，无命令选择器）、关闭（`pty.close`，无二次确认——会话粒度闸已有）、切换。
- 选中的 session 住在 `DashboardState` 新信号 `terminal_selection: RwSignal<Option<String>>`（会话级事实，多设备不共享 ⇒ 不进 localStorage 也不进服务端）。

**面板 → 终端连线**（D3）：`AgentRow` 加 `on:click`：写 `terminal_selection` + 切 `PanelMode::Terminal`。TUI 无终端渲染，不做。

**粘贴**（D11）：canvas 挂 `on:paste`，读 `clipboardData.getData("text")`，按最近一帧的 `bracketed_paste` 决定是否包 `ESC[200~ … ESC[201~`，走 `pty.input`。快捷键：Cmd-V（mac）与 Ctrl-Shift-V（其余）**不** `prevent_default`，放浏览器触发 paste 事件；裸 Ctrl-V 仍是 0x16（终端惯例）。

**光标**：`cursor_visible == Some(false)` 时不画光标。

**失败态**（D8）：Panel 按 RPC 错误码分「被拒 / 不可用」，与 TUI 同一分类；测试 `refused_and_unavailable_render_differently` 的 Panel 版。

**共享化**（S6/S4/S5）：`state_glyph()` 搬进 `shared/ui_logic::agent_panel`，两端调用；CUT `AgentPanelState::collapsed`；两端渲染 `program`（或 `agent`）与 `quiet_since` 的年龄（`"quiet 3m"`）。

### 4.4 W-D · `terminal` 工具增量 — `src/builtin_tools/terminal.rs`

- `terminal{wait, session_id, until?: ["blocked"|"idle"|"working"|"unknown"], timeout_ms?}`：服务端等待，靠 `RuntimeAgents` 新增的 `tokio::sync::watch<u64>` 代数号（任何 `changed` 递增）唤醒；`timeout_ms` 上限 150,000（`WAIT_MAX_TIMEOUT_SECS=170` 是硬约束，R10）；返回终态 entry 或 `timeout`。**不轮询屏幕**。
- `terminal{explain, session_id}`：返回 `agent_detect` 的 `DetectionExplain`（命中规则、输入摘要、`manifest_version`）——herdr `agent explain` 的对应物，也是 G3（规则年龄不可见）的合法消费者。
- `DESCRIPTION` 增补两句归这两个动词所有（R9 第二把尺）。
- **D7 修法**：`ambient_actor()` 为 `None` 时，`list/status` 只放行 `created_by == None` 的行，`read` 对 `created_by.is_some()` 的会话答 `no_such_session`。守卫：零身份 + 一条 `created_by = Some(x)` 的会话 ⇒ 三个动词都看不见它。
- R11-14 的标签缺陷（审批卡说"changes configuration"）**本轮不动**——两处注释禁止朴素修法，且它不在本轮路径上。

### 4.5 W-0 · wire 先行（单独一笔提交，其他并行流的基线）

`shared/protocol`：
- `pty.rs`：`PtySessionInfo { session_id, shell, cwd, created_at, closed }`、`PtyListResponse { sessions }`、`PTY_LIST_METHOD`；**不带** `created_by`（D9a，零读者）。`PtyScreenPatch` 加 `cursor_visible`、`bracketed_paste`（`Option<bool>`，`skip_serializing_if`）。
- `runtime.rs`：`program`、`quiet_since`。
- 服务端 `handle_list` 用类型构造；`terminal{list}` 同；键集相等断言（判据 §10）。Panel 用类型解析。
- 此笔之后 `PtyScreenPatch` 的两个新字段暂由服务端恒发 `None`，`program`/`quiet_since` 暂发 `None`——**每个占位都有一条注释指向接线任务**，并由 W-A/W-B 的守卫在接线后变红证明接上了。

### 4.6 W-E · 真机 QA — `qa/terminal/run.sh`

沿用 `qa/lib` 的起服/假 key/`tools.invoke` 装置（记忆：没有假 key 时 `tools.invoke` 是占位符）。阶段：
- `identify`：`pty.spawn` 一个 shell → `pty.input` 敲 `fake-claude`（装置自带脚本，画 `claude.toml` 的 idle→working→blocked chrome，各停 2s）→ 轮询 `runtime.agents.list`，断言 `agent`、`program`、`state` 三段依次出现。**这是 D1 在发货二进制上的证明。**
- `wait`：`tools.invoke terminal{wait, until:["blocked"]}` 在 blocked 阶段之前发出，断言它在阶段到达时返回且耗时落在窗口内。
- `quiet`：脚本进入 working 后停止输出 35s，断言 `quiet_since` 出现而 `state` 仍是 `working`。
- `cwd`：脚本发 `OSC 7`，断言 `cwd` 跟着变。

⚠️ 共享 `target/` 时 `target/debug/aleph-server` 是最后一次构建的那棵树的二进制（记忆 `project-ratchet-attribution-and-fixture-blindspot`）——装置必须在本 worktree 里**先构建再跑**，并把二进制路径打印出来。

### 4.7 W-F · 文档

- `docs/reference/FEATURE_LOCATOR.md`：新 **§6.11 Panel 内嵌终端与 PTY 基座**（补 08-29 那轮欠的 Phase 8）与 **§6.12 运行时 agent 面板（herdr 移植 1+2 期）**；附录 E.4 / E.7 各加本轮触发器；附录 D 加「一条注释里写着 KNOWN GAP 的断线，dead-code 与守卫都看不见」「守卫的作用域是函数而读者当它是功能」两条全文。
- `docs/reference/TERMINAL_RUNTIME.md`（新，Tier 2）：架构 · 数据流 · 状态仲裁与去抖常量 · 两道闸 + 工具面授权 · wire 契约 · herdr 对照表与**刻意不做清单**。
- `CLAUDE.md`：子系统路由表 `src/gateway/pty/` 行的 `FL §6.11` 指针变成真的；新增 `src/gateway/runtime/` · `crates/agent-detect/` · `src/builtin_tools/terminal.rs` 一行（`FL §6.12`，QA `qa/terminal/run.sh`）。**只增行，不加话术。**
- `SECURITY.md` 内嵌终端段：工具面四个动词 + D7 的零身份臂；`TOOL_SYSTEM.md`：`terminal` 条目。
- 0-A 清单：A2–A8/B1/C1/C5/C9 标 SHIPPED；第 1 期 spec 顶部加一行状态指针（不改正文——两份表述）。

---

## 5. 错误与降级

| 情况 | 正确行为 | 错误行为 |
|---|---|---|
| 探测不到前台进程（权限/平台） | `program: None`，识别退回 shell 标签 ⇒ 多半 `Unknown` | 猜一个 |
| 探测到的程序不在表里（`vim`） | `program: Some("vim")`，`agent: None`，`Unknown` | 显示成 idle |
| agent 退出、shell 回到前台 | 6 次 miss 后 `program` 回 shell、`agent` 变 None、state `Unknown` | 立刻抖动 |
| 30s 无帧 | `quiet_since: Some(t)`，state 不变 | 变 idle |
| OSC 7 带非本机 host | 丢弃，cwd 走下一来源 | 当路径用 |
| `terminal{wait}` 超时 | 返回 `timeout` + 当前 entry | 返回最后一次 entry 当作终态 |
| 零身份调用 `terminal{read}` 有归属会话 | `no_such_session`（与不存在同字） | 返回屏幕 |
| Panel 收到 `bracketed_paste` 之前就粘贴 | 不包裹（`None` = 不知道 ⇒ 按最弱假设） | 假定开启 |

---

## 6. 验证集

在 CLAUDE.md 六条之外：

```
cargo test -p agent-detect
cargo test -p aleph-protocol
cargo test -p alephcore --lib gateway::pty gateway::runtime builtin_tools::terminal
cargo test -p aleph-ui-logic
cargo test -p aleph-tui -p aleph-cli
cargo test -p aleph-panel --lib          # webchat 有改动就跑
just wasm                                 # 出厂形态
./qa/terminal/run.sh {identify,wait,quiet,cwd}
```

守卫证伪清单（每条实施后手动剪一次，记录在 plan 里）：① 剪掉 `foreground` 探测 ⇒ `a_real_agent_started_after_spawn_is_identified` 红；② 剪掉 `quiet_since` 翻转 ⇒ quiet 守卫红；③ 剪掉滚动区读取 ⇒ `CSI r` 对比测试红；④ 剪掉 `on:click` ⇒ Panel tab 模型测试红（模型层）；⑤ 零身份臂放宽 ⇒ D7 守卫红。

---

## 7. DECIDE — 需要用户裁定、本轮不实施

### 7.1 `terminal` 写动词（`spawn` / `send` / `keys` / `close`）

**为什么不能由本轮自作主张**：PTY 不经 `[sandbox.command_policy]` 也不经 exec tier（SECURITY.md「内嵌终端」段自陈）。今天这条路只对**人**开放（Panel）。把它给 LLM 就是给 Aleph 自己的模型一个绕过全部命令闸的 shell——这是授权架构的决定，不是功能。

**若裁定「做」，推荐形状**（全部取自 herdr `agent prompt` 路径，`src/app/api/agents.rs:63-130`，并用 Aleph 已有的东西超越它）：
1. 只做 `send`（提示词）与 `keys`（`esc`/`ctrl+c` 这类逻辑键），**不做 `spawn`/`close`** 第一版——起终端仍由人做，模型只能对**已识别为 agent** 的会话说话。
2. `send` 的前置检查，任一失败都在写第一个字节之前拒绝：非空 · 目标已识别（`agent.is_some()`）· 目标**不是** `Blocked`（blocked 会话在显示模态，文字进去意义不同）· 探测确认目标**仍是**前台进程 · 文本按 `bracketed_paste` 实时模式包裹 · Enter 延迟 300ms 单独发送。
3. `keys` 跳过 Blocked 检查（它就是用来回答模态的），但**整批先校验再发**。
4. **超越 herdr 的那一点**：herdr 协议层零身份，靠 SKILL.md 的散文约束调用者；Aleph 有 Ed25519 agent 身份 + `owner_admits` ——写动词只对 `owner_admits(created_by, actor)` 为真的会话开放，且每次写入进 `AGENT_IDENTITY` 的签名账本。
5. 加一根会话旋钮 `terminal_write: off|ask|on`（默认 `off`；`ask` 走现有审批卡，先修 R11-14 的标签缺陷）。

### 7.2 BSP 分屏（08-29 spec Phase 6）

tab 之后的自然下一步；本轮不做的唯一理由是 Panel 端零测试环境 + 规模。若做：布局树按 08-29 spec §6.2，几何**派生不存储**（herdr `src/layout.rs:126`），ratio 以 root 路径寻址。

### 7.3 agent hooks 状态源（herdr §3）

需要往用户的 `~/.claude/settings.json` 等配置里写 hook——这是对用户环境的写入，要用户点头。herdr 的经验：**只对能表达 blocked/interrupt 的 agent 做全生命周期 hook**（6 个），其余只收 session id。

---

## 8. 明确不做（第 3 期候选）

BSP 分屏 · scrollback 读取 RPC + 滚轮（D13）· 鼠标模式（D12/C6）· kitty 键盘（C4）· 会话持久化/恢复与 fd 交接（herdr §5）· agent hooks（§7.3）· `seen`/`Done` 第五态与注意力流（需要写入面）· worktree 模型 · manifest 远端热更新 · `AgentPanelState` 的 leptos 信号面（G4，已裁不需要）· TUI 拖拽分割条（G1，spec 改口即可）。

---

## 9. 风险

- **最大**：`sysinfo` 单 pid 刷新在 macOS 上的成本。已知 `utils/process_alive.rs` 在用同一形状，但那是低频；本轮限频 500ms/3s 且有计数守卫。若真机 QA 显示 CPU 抬头，退到「只在屏幕变化 tick 探测」。
- **次大**：滚动区改动波及 `Grid` 五处调用点，任何一处漏读区域就是 `visible_text()` 对不上任何一帧。用「有它/没它」对比测试和 `vim` 样例顶住。
- **已知不覆盖**：Windows 的前台进程只有后代启发式，没有真机验证；`cargo check -p aleph-desktop-windows` 只能证明它编译。

---

## 10. 自审记录（2026-09-04）

| 类型 | 发现 | 处置 |
|---|---|---|
| 占位符 | §4.1 探测常量原写「若干」 | 给出数值并注明来源（herdr 常量）或本轮裁定 |
| 内部矛盾 | §1 表说「做 tab」但 §4.3 曾写"tab + 分屏" | 统一为 tab；分屏进 §7.2 |
| 悬空的写者 | `quiet_since` 若进 `changed` 会每帧触发事件 | 改为只在 None↔Some 翻转时算 changed |
| 歧义 | `cwd` 三个来源谁压谁 | 固定顺序 OSC 7 › 前台 › spawn，写进 wire 文档 |
| 范围 | W-D 的 D7 修法会不会挡住 loopback operator 自己的会话 | 实施任务第一步先核实 loopback operator 的 `ambient_actor()` 是否为 `None`；若 Panel 起的会话 `created_by` 也是 `None`，修法无副作用；若不是，改为「零身份臂只放行 `created_by == None`」仍成立——两种情况都不会挡住有身份的调用者 |
