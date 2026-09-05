# 多 agent 终端窗口 · 第二轮 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让第 1 期交付的 agent 面板在生产上真的识别出 agent，把 Panel 终端做成多 tab 并与面板连线，补掉 0-A Tier A 的 VT 缺口，给 `terminal` 工具加两个只读观察动词。

**Architecture:** 服务端持屏不变；新增前台进程探测（`portable-pty` + `sysinfo`）作为识别源，OSC 7 / 前台进程 cwd 作为 live cwd，`quiet_since` 作为静默事实；wire 契约先行落地，三条实现流各占一棵 worktree 并行，合并后再做依赖它们的工具面与真机 QA。

**Tech Stack:** Rust (tokio, portable-pty 0.8, sysinfo 0.39, vte 0.14), Leptos 0.8 (WASM), ratatui, bash QA 装置。

**Spec:** `docs/superpowers/specs/2026-09-04-terminal-round2-design.md`（本计划从它论证；实施者两份都读）

## Global Constraints

- 禁止引入第二个 VT 实现；所有 VT 能力扩容只在 `src/gateway/pty/screen/`（CLAUDE.md 禁用清单）。
- `src/` 不得直接依赖平台 API crate；进程事实只用 `portable-pty` 与 `sysinfo`（spec R2-2）。
- 不进 `src/harness/`（R10）；`WAIT_MAX_TIMEOUT_SECS=170` 是硬约束，`terminal{wait}` 的 `timeout_ms` 上限 150,000。
- 服务端响应必须用 `shared/protocol` 类型构造，不许手搓 `json!`（判据 §10）。
- 静默不等于 idle：任何"按时间把 Working 变 Idle"的代码都是违规（spec R2-3）。
- 一棵 worktree 同一时刻只有一个 agent 在改（spec R2-7）；三条并行流的文件集见 §文件结构，越界即冲突。
- 共享 `target/`：QA 必须在本 worktree 先构建再跑，并打印二进制路径。
- 提交信息英文、`<scope>: <description>`；每个任务结束一次提交。
- **计划形态说明**：本计划给出精确的跨任务接口签名、测试名与验证命令；实施者要先读所在模块的现有代码再写实现，不得把这里的签名当成可以不看上下文直接粘贴的代码。

---

## 文件结构（谁动哪些文件）

| 流 | 只允许改动 |
|---|---|
| **Task 0（wire 先行，主 worktree，串行）** | `shared/protocol/src/{pty.rs,runtime.rs}` · `src/gateway/handlers/pty.rs`（`handle_list`）· `src/gateway/pty/manager.rs`（`SessionInfo` 投影）· `src/gateway/pty/screen/convert.rs` · `src/gateway/runtime/mod.rs`（占位）· `src/builtin_tools/terminal.rs`（`list`）· `interfaces/webchat/src/platform/wide/views/terminal/mod.rs`（`pty.list` 解析）· 所有构造 `RuntimeAgentEntry` 字面量的测试文件 |
| **Stream A（识别）** | `src/gateway/pty/foreground.rs`（新）· `src/gateway/pty/{mod,session,manager}.rs` · `src/gateway/runtime/{mod,tests}.rs` · `crates/agent-detect/src/{lib,engine}.rs` |
| **Stream B（VT）** | `src/gateway/pty/screen/**` 全部 |
| **Stream C（前端）** | `interfaces/webchat/src/**`（不含 `api.rs` 的 RPC 方法名常量以外的地方随意）· `shared/ui_logic/src/state/agent_panel*.rs` · `interfaces/tui/src/tui/widgets/agent_panel.rs` · `interfaces/webchat/locales/{en,zh}.json` |
| **Task M（合并 + 胶水，主 worktree）** | 合并三条流；`src/gateway/runtime/mod.rs` 读 `screen.cwd()` |
| **Task D（工具面）** | `src/builtin_tools/terminal.rs` · `src/executor/builtin_registry/definitions.rs`（schema）· `src/gateway/runtime/mod.rs`（只消费 A 暴露的 `subscribe()`）|
| **Task E（QA）** | `qa/terminal/**`（新）· `qa/README.md` |
| **Task F（文档）** | `docs/reference/{FEATURE_LOCATOR,TERMINAL_RUNTIME,SECURITY,TOOL_SYSTEM}.md` · `CLAUDE.md` 路由表 · `docs/superpowers/plans/2026-09-03-0a-vt-capability-gaps.md` · 第 1 期 spec 顶部一行 |

---

### Task 0: wire 契约先行

**Files:**
- Modify: `shared/protocol/src/pty.rs`（`PtyScreenPatch` 加字段；新增 `PtySessionInfo`、`PtyListResponse`、`PTY_LIST_METHOD`）
- Modify: `shared/protocol/src/runtime.rs`（`RuntimeAgentEntry` 加 `program`、`quiet_since`）
- Modify: `src/gateway/pty/manager.rs:56-69`（`SessionInfo` → `impl From<&SessionInfo> for PtySessionInfo`，**不带** `created_by`）
- Modify: `src/gateway/handlers/pty.rs:386-389`（`handle_list` 用 `PtyListResponse` 构造）
- Modify: `src/builtin_tools/terminal.rs:214-227`（`list` 用同一类型）
- Modify: `src/gateway/pty/screen/convert.rs`（新字段透传，本任务恒 `None`）
- Modify: `src/gateway/runtime/mod.rs:218-241`（`program: None`, `quiet_since: None`，各带一行注释 `// wired by Stream A, guarded by <test name>`）
- Modify: `interfaces/webchat/src/platform/wide/views/terminal/mod.rs:133-152`（`serde_json::from_value::<PtyListResponse>`）
- Modify: 所有 `RuntimeAgentEntry { .. }` 字面量（`shared/protocol`、`shared/ui_logic`、`interfaces/tui`、`interfaces/webchat`、`src/gateway`）——用 `grep -rn "RuntimeAgentEntry {" --include=*.rs` 数，**先数再改**（判据 §6）

**Interfaces（Produces）:**
```rust
// shared/protocol/src/pty.rs
pub const PTY_LIST_METHOD: &str = "pty.list";
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtySessionInfo { pub session_id: String, pub shell: String, pub cwd: String, pub created_at: i64, pub closed: bool }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyListResponse { pub sessions: Vec<PtySessionInfo> }
// PtyScreenPatch 新增（放在 alt_screen 旁，同样 "Some 只在变化时"）：
pub cursor_visible: Option<bool>,
pub bracketed_paste: Option<bool>,
pub cwd: Option<String>,          // OSC 7，Stream B 供给；Panel 本轮不渲染，tab 悬停可用
// shared/protocol/src/runtime.rs
pub program: Option<String>,      // 前台程序名；None = 探不到
pub quiet_since: Option<i64>,     // ms；Some = 连续 QUIET_AFTER_MS 无帧
```

- [ ] **Step 1: 写红测试** — `shared/protocol/src/pty.rs`：`pty_list_response_round_trips_and_pins_its_key_set`（用类型构造 → `to_value` → 断言 `sessions[0]` 的键集合 == `{"session_id","shell","cwd","created_at","closed"}`，且没有 `created_by`）；`src/gateway/handlers/pty.rs`：`list_response_is_built_from_the_protocol_type`（起真 session，调 `handle_list`，`from_value::<PtyListResponse>` 成功且 `sessions.len()==1`）。
- [ ] **Step 2: 跑，确认红**（类型不存在）。
- [ ] **Step 3: 实现** 上面接口；`handle_list` 与 `terminal{list}` 都经 `PtySessionInfo::from`；Panel 改用类型解析；`convert.rs` 三个新字段填 `None`；`runtime/mod.rs` 两个占位。
- [ ] **Step 4: 全绿** — `cargo test -p aleph-protocol && cargo test -p alephcore --lib gateway::pty gateway::runtime builtin_tools::terminal && cargo test -p aleph-ui-logic && cargo test -p aleph-tui && cargo test -p aleph-panel --lib`。
- [ ] **Step 5: 提交** `protocol,gateway,panel: share the pty.list type and reserve the round-2 wire fields`。

---

### Stream A · Task A1: 前台进程探测

**Files:**
- Create: `src/gateway/pty/foreground.rs`
- Modify: `src/gateway/pty/mod.rs`（`pub mod foreground;`）
- Modify: `src/gateway/pty/session.rs:91-130`（`PtySession` 加 `foreground: Mutex<ForegroundState>`；`pub fn foreground_fact(&self) -> Option<ForegroundFact>`；`pub fn shell_pid(&self) -> Option<u32>`）
- Modify: `src/gateway/pty/manager.rs:209-224`（`flush_session` 在 take_patch 之后、sample 之前调 `session.maybe_probe_foreground(now, frame_produced)`，**在 screen 锁之外**）
- Modify: `crates/agent-detect/src/{lib,engine}.rs`（`identify_agent_from_process`）

**Interfaces（Produces）:**
```rust
// src/gateway/pty/foreground.rs
pub const PROBE_MIN_INTERVAL_MS: i64 = 500;
pub const PROBE_RECHECK_MS: i64 = 3_000;
pub const PROBE_MISSES_TO_FORGET: u32 = 6;   // herdr AGENT_MISS_CONFIRMATION_ATTEMPTS
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForegroundFact { pub pid: u32, pub name: String, pub argv0: Option<String>, pub cmdline: Option<String>, pub cwd: Option<String>, pub observed_at: i64 }
/// 是否该在这个 tick 探测。纯函数，可单测。
pub fn probe_due(last_probe_at: Option<i64>, now: i64, frame_produced: bool, agent_known: bool) -> bool;
/// 平台探测。Unix: master.process_group_leader() + sysinfo 单 pid；非 Unix: 后代启发式。
pub fn probe(master: &dyn portable_pty::MasterPty, shell_pid: Option<u32>) -> Option<ForegroundFact>;
/// 计数守卫用：本进程累计的 sysinfo 刷新次数（thread-local 或 AtomicU64）。
pub fn probe_count() -> u64; pub fn reset_probe_count();
// PtySession
pub fn maybe_probe_foreground(&self, now: i64, frame_produced: bool, agent_known: bool);
pub fn foreground_fact(&self) -> Option<ForegroundFact>;   // 经过 6-miss 滞后之后的"当前认定"
// crates/agent-detect
pub fn identify_agent_from_process(name: &str, argv0: Option<&str>, cmdline: Option<&str>) -> Option<Agent>;
```

- [ ] **Step 1: 红测试**（`foreground.rs` 内）：`probe_due_respects_min_interval_and_recheck`（表驱动四种组合）；`a_real_child_is_reported_as_the_foreground_program`（`#[cfg(unix)]`：真 PTY spawn `sh -c 'exec sleep 30'`… 或 spawn `sleep 30` 直接为 child，断言 `probe(..).name == "sleep"`，结束后 kill）；`misses_are_hysteretic_hits_are_immediate`（模拟 `ForegroundState::observe(Some/None)` 六次）；`identify_agent_from_process_reads_node_scripts`（`("node", Some("node"), Some("/usr/local/bin/claude --resume x")) → Some(Agent::Claude)`；`("vim", ..) → None`）；计数守卫 `probe_count_can_reach_one`。
- [ ] **Step 2: 跑，确认红。**
- [ ] **Step 3: 实现。** `sysinfo` 用 `System::new()` + `refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, ProcessRefreshKind::nothing().with_cmd(..).with_exe(..).with_cwd(..))`（沿用 `src/utils/process_alive.rs::with_process` 的形状；若该 helper 私有则在那边 `pub(crate)` 化并复用，**不要抄第二份**）。`identify_agent_from_process`：移植 herdr `src/detect/mod.rs:243-271` 的 `normalized_process_name`/`process_priority` 逻辑（保留 Apache-2.0 头），basename 归一化后先 `name`，再 `argv0`，再 cmdline 的第二个 token（node/python 脚本）。
- [ ] **Step 4: 全绿** — `cargo test -p agent-detect && cargo test -p alephcore --lib gateway::pty`。跑 `cargo check -p aleph-desktop-windows` 证明 `cfg(not(unix))` 臂编译（若该 crate 不编译 alephcore，则至少 `cargo check -p alephcore --target` 不可用时写明未验证）。
- [ ] **Step 5: 提交** `gateway/pty: probe the foreground process behind a rate gate and a miss hysteresis`。

### Stream A · Task A2: 采样器接线 + quiet_since + 端到端守卫

**Files:**
- Modify: `src/gateway/runtime/mod.rs`（`sample` 签名、`identify` 前置、`agent_changed` 项、`quiet_since`、`subscribe()`）
- Create: `src/gateway/runtime/tests.rs`（把 910 行测试搬出去，`#[cfg(test)] mod tests;`）
- Modify: `src/gateway/pty/manager.rs:601-623`（把 `session.foreground_fact()` 与 `frame_produced` 喂给 sample；`release_expired` 之后调 `mark_quiet(now)`）
- Modify: `crates/agent-detect/src/manifests/claude.toml`（**不改**；测试脚本要匹配它现有的某条 `working` 规则——先读规则再写脚本）

**Interfaces:**
```rust
pub const QUIET_AFTER_MS: i64 = 30_000;
pub struct SampleInput<'a> { pub session_id: &'a str, pub shell: &'a str, pub program: Option<&'a str>, pub argv0: Option<&'a str>, pub cmdline: Option<&'a str>, pub cwd: &'a str, pub screen: &'a Screen, pub process_exited: bool, pub frame_produced: bool, pub now: i64 }
impl RuntimeAgents {
    pub fn sample(&self, input: SampleInput<'_>) -> bool;          // changed?
    pub fn mark_quiet(&self, now: i64) -> Vec<String>;             // 翻转了 quiet_since 的 session ids（每 tick）
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64>;  // 任何 changed / release / quiet 翻转 / remove 都递增
}
```
`cwd` 由 manager 决定：`screen.cwd()`（Task M 接）∨ `foreground_fact.cwd` ∨ `session.cwd()`。

- [ ] **Step 1: 红测试**（`runtime/tests.rs`）：`a_real_agent_started_after_spawn_is_identified`（真 PTY 起 `sh`，写入 `export PATH=<tmp>:$PATH; claude\n`，`<tmp>/claude` 是可执行脚本，输出 `claude.toml` 某条 `working` 规则匹配的画面并 `sleep 5`；轮询 ≤ 3s 直到 `snapshot()` 里该 session `agent == Some("claude")` 且 `program == Some("claude")` 且 `state == Working`；serial 标签同 `pty` 测试族）；`a_quiet_working_agent_reports_quiet_since_without_becoming_idle`（纯函数：sample Working → `mark_quiet(now+31_000)` 返回该 id，entry.quiet_since == Some(now)，state 仍 Working；再 sample(frame_produced) → quiet_since None）；`quiet_flip_is_a_change_but_quiet_value_is_not`；`agent_change_clears_the_idle_hold`；`subscribe_bumps_on_every_observable_change`；`identify_runs_before_the_screen_text_is_built`（用一个会 panic 的 Screen 桩？——改为计数：`Screen::visible_text` 调用计数在 `program=None && shell="zsh"` 时为 0）；`probe_count_is_bounded_at_fifteen_sessions`（15 个 session × 100 tick 的 `probe_due` 真值数 ≤ 15×(100/ (500/16)) 的上界，用 `probe_count`）。
- [ ] **Step 2: 红。**
- [ ] **Step 3: 实现。** 删除 `mod.rs:99-131` 那段 KNOWN GAP 注释（缺口已合）；D10 那条 `the_osc_progress_wire_is_actually_connected` 保留但改 doc：作用域是函数，端到端由新守卫证明。
- [ ] **Step 4: 证伪**：注释掉 manager 里喂 `foreground_fact` 的那一行，`a_real_agent_started_after_spawn_is_identified` 必须红；恢复。记录在提交信息里。
- [ ] **Step 5: 全绿** `cargo test -p alephcore --lib gateway::runtime gateway::pty`。
- [ ] **Step 6: 提交** `gateway/runtime: identify agents from the foreground process, publish quiet_since, expose a change watch`。

---

### Stream B · Task B1: `screen/` 重排 + 无状态序列

**Files:**
- Modify: `src/gateway/pty/screen/perform.rs` → 拆为 `perform/{mod,csi,esc,osc}.rs` + `perform/tests.rs`（生产 ≤ 500 行/文件）
- Modify: `src/gateway/pty/screen/grid.rs`（`autowrap`、`last_printed`）

- [ ] **Step 1: 先做纯搬家**（不改行为）：`cargo test -p alephcore --lib gateway::pty::screen` 前后条数相等；提交 `gateway/pty/screen: split perform.rs by dispatch face, move tests out`。
- [ ] **Step 2: 红测试**（每条都是"同输入，有/无该序列，`visible_text()` 不同"）：`ris_clears_grid_title_and_saved_cursor`（且 scrollback **保留**，写明）；`decstr_resets_modes_but_keeps_the_grid`；`decawm_off_overwrites_the_last_column_instead_of_wrapping`（且 `cursor_col < cols` 恒成立）；`ind_moves_down_same_column_nel_moves_down_col_zero`；`rep_repeats_the_last_printed_char_and_a_control_byte_invalidates_it`；`legacy_alt_screen_47_and_1047_keep_the_alt_grid_across_exit`（与 1049 对比）；`mode_1048_saves_and_restores_the_cursor`；`osc_title_with_semicolons_is_rejoined`（`\e]0;a;b;c\a` → `"a;b;c"`）；`dcs_hook_put_unhook_are_explicit_no_ops`（喂 `\eP…\e\\` 后 `visible_text()` 不变）。
- [ ] **Step 3: 红 → 实现 → 绿。**
- [ ] **Step 4: 提交** `gateway/pty/screen: RIS/DECSTR, DECAWM, IND/NEL, REP, legacy alt screen, OSC title rejoin, explicit DCS no-ops`。

### Stream B · Task B2: 滚动区 + 原点模式

**Files:** `grid.rs`（`scroll_region: (u16,u16)`, `origin_mode: bool`, `scroll_down`, 五处调用点改读区域）· `perform/csi.rs`（`r`, `S`, `T`, `?6`）· `perform/esc.rs`（`M`）

- [ ] **Step 1: 红测试**：`decstbm_scrolls_only_the_region_and_pins_the_header`（`CSI 2;23r`，在底行连发 `\n`，首行文本不动）；`rows_leaving_a_region_top_do_not_enter_scrollback`；`reverse_index_at_region_top_scrolls_down`；`su_and_sd_scroll_within_the_region`；`origin_mode_makes_cup_relative_to_the_region`；`resize_and_ris_reset_the_region`；`insert_and_delete_lines_respect_the_region`；`erase_in_display_within_a_region_is_still_screen_absolute`（ED 是屏幕绝对的——写明为什么）。
- [ ] **Step 2–3: 红 → 实现 → 绿。**
- [ ] **Step 4: 证伪**：把 `newline()` 的区域读取换回 `0..rows`，`decstbm_…` 必红；恢复。
- [ ] **Step 5: 提交** `gateway/pty/screen: scroll regions (DECSTBM/SU/SD/RI) with origin mode`。

### Stream B · Task B3: 可观测模式位与 OSC 7

**Files:** `perform/csi.rs`（`?25`, `?2004`）· `perform/osc.rs`（OSC 7）· `perform/mod.rs`（`ScreenState` 三字段 + `take_patch` 的"变化才 Some"）· `convert.rs` · `screen/mod.rs`（`Screen::cwd()`, `cursor_visible()`, `bracketed_paste()`）

- [ ] **Step 1: 红测试**：`cursor_visibility_rides_the_patch_only_when_it_changes`；`bracketed_paste_mode_rides_the_patch`；`osc7_file_uri_with_empty_or_localhost_host_sets_cwd_and_percent_decodes`；`osc7_with_a_foreign_host_is_dropped`；`attach_snapshot_carries_the_current_mode_bits`（重连后 Panel 要拿到全量）。
- [ ] **Step 2–3: 红 → 实现 → 绿。**
- [ ] **Step 4: 提交** `gateway/pty/screen: track DECTCEM, bracketed paste and OSC 7 cwd, publish them on the patch`。

---

### Stream C · Task C1: 共享字形 + 失败态分类 + CUT

**Files:** `shared/ui_logic/src/state/agent_panel.rs`（`pub fn state_glyph(RuntimeAgentState) -> &'static str`; CUT `collapsed`）· `agent_panel_parity.rs`（`glyphs_are_distinct_and_unknown_is_not_idle`）· `interfaces/tui/src/tui/widgets/agent_panel.rs`（改用共享字形；保留颜色）· `interfaces/webchat/src/components/sidebar/agent_panel.rs`（改用共享字形；`error` 拆成 `refused: Option<String>` / `unavailable: Option<String>`，按 RPC 错误码分，与 TUI `agent_panel_data` 同一分类：`AUTH_REQUIRED` 码 → refused，其余 → unavailable）

- [ ] **Step 1: 红测试**：ui_logic `glyphs_are_distinct_and_unknown_is_not_idle`；Panel `refused_and_unavailable_render_differently`（组件级：两种 `Err` 渲染出的 class 不同）；TUI 既有 `unknown_never_wears_idles_glyph` 改为断言等于 `ui_logic::state_glyph(Unknown)`。
- [ ] **Step 2–3: 红 → 实现 → 绿** `cargo test -p aleph-ui-logic -p aleph-tui && cargo test -p aleph-panel --lib`。
- [ ] **Step 4: 提交** `ui-logic,tui,panel: one glyph table, a refused/unavailable split in the panel, cut the unread collapsed flag`。

### Stream C · Task C2: tab 条 + 面板 → 终端连线

**Files:**
- Modify: `interfaces/webchat/src/context.rs`（`DashboardState.terminal_selection: RwSignal<Option<String>>`）
- Create: `interfaces/webchat/src/platform/wide/views/terminal/tabs.rs`（纯模型 `TabModel` + `TabBar` 组件）
- Modify: `views/terminal/mod.rs`（用 `terminal_selection` 决定 attach 哪个 session；新建/关闭走 `pty.spawn`/`pty.close`；订阅 `pty.exit` 更新模型；`ClientScreen::title()` 喂 tab 标题）
- Modify: `components/sidebar/agent_panel.rs:259-282`（`AgentRow` 加 `on:click`：`terminal_selection.set(Some(id))` + `mode.set(PanelMode::Terminal)`）
- Modify: `locales/{en,zh}.json`（`terminal.new_tab`, `terminal.close_tab` 等，先跑 `i18n_census` 看守卫怎么数）

**Interfaces:**
```rust
pub struct TabEntry { pub session_id: String, pub title: String, pub state: Option<RuntimeAgentState>, pub program: Option<String>, pub closed: bool }
pub struct TabModel { tabs: Vec<TabEntry>, selected: Option<String> }
impl TabModel {
    pub fn reconcile(&mut self, sessions: &[PtySessionInfo], agents: &[RuntimeAgentEntry]);  // 合并、保持顺序、丢 closed
    pub fn on_exit(&mut self, session_id: &str);
    pub fn on_title(&mut self, session_id: &str, title: &str);
    pub fn select(&mut self, session_id: &str) -> bool;   // false = 不存在
    pub fn selected(&self) -> Option<&TabEntry>;          // 选中项被关闭 ⇒ 落到相邻项
}
```

- [ ] **Step 1: 红测试**（`tabs.rs` 纯模型，无 DOM）：`reconcile_joins_agent_state_by_session_id`；`closing_the_selected_tab_falls_to_a_neighbour`；`select_an_unknown_session_is_refused_not_silently_added`；`title_prefers_osc_then_program_then_shell`；`agent_row_click_selects_the_session_and_switches_mode`（组件测试：模拟 click 后两个信号的值）。
- [ ] **Step 2–3: 红 → 实现 → 绿** `cargo test -p aleph-panel --lib`。
- [ ] **Step 4: 出厂形态** `just wasm`（唯一编译出厂形态的命令）。
- [ ] **Step 5: 提交** `panel: terminal tabs over pty.list, and the agent-panel row opens its session`。

### Stream C · Task C3: 粘贴、光标可见性、面板年龄

**Files:** `views/terminal/{mod,keymap,session,render}.rs` · `components/sidebar/agent_panel.rs` · `interfaces/tui/src/tui/widgets/agent_panel.rs` · `shared/ui_logic/src/state/agent_panel.rs`（`pub fn quiet_label(quiet_since: Option<i64>, now: i64) -> Option<String>`：`"quiet 3m"` 的唯一拼法）

- [ ] **Step 1: 红测试**：keymap `cmd_v_and_ctrl_shift_v_are_left_to_the_browser_ctrl_v_is_0x16`（返回 `KeyAction::Browser` vs `Bytes`）；session `paste_wraps_when_bracketed_paste_is_on_and_not_when_unknown`（纯函数 `encode_paste(text, bracketed: Option<bool>) -> Vec<u8>`）；session `cursor_visible_false_is_stored_and_render_skips_the_cursor`；ui_logic `quiet_label_is_none_when_active_and_rounds_down`。
- [ ] **Step 2–3: 红 → 实现 → 绿**。两端面板行渲染 `program`（无则 `agent`）与 `quiet_label`。
- [ ] **Step 4: 提交** `panel,tui: paste via the clipboard event, honour cursor visibility, show program and quiet age`。

---

### Task M: 合并三条流 + 胶水

- [ ] **Step 1**: 在主 worktree `git merge` 三条流的分支（顺序 B → A → C），冲突只应出现在 `shared/protocol/src/pty.rs` 与 `interfaces/webchat/.../terminal/mod.rs`；解决后**先看警告再看错误**（`unused` 说明某半边没有调用者，正解是 CUT）。
- [ ] **Step 2**: 胶水——`manager.rs` 的 cwd 来源顺序 `screen.cwd()` › `foreground_fact.cwd` › `session.cwd()`；红测试 `cwd_prefers_osc7_then_foreground_then_spawn`。
- [ ] **Step 3**: 最小可信验证集六条 + spec §6 的增量；`cargo clippy --workspace --all-targets`（先 `just _stage-shell-placeholders`）。
- [ ] **Step 4: 提交** `merge: terminal round-2 streams A/B/C; cwd source order`。

### Task D: `terminal{wait,explain}` + 零身份臂

**Files:** `src/builtin_tools/terminal.rs` · `src/executor/builtin_registry/definitions.rs`（schema `action` 枚举加 `wait`/`explain`；`until`、`timeout_ms`）· `src/gateway/method_authz.rs`（若 `MUST_STAY_GATED` 按动词计数需更新）

**Interfaces:**
```rust
// terminal{wait}: params { session_id, until?: Vec<RuntimeAgentState> (默认 [blocked, idle]), timeout_ms?: u64 (≤150_000, 默认 60_000) }
// 返回 { outcome: "reached" | "timeout" | "gone", agent: RuntimeAgentEntry? }
// terminal{explain}: params { session_id } → { agent, state, matched_rule, source, manifest_version, inputs: { title, osc_progress, screen_tail } }
```

- [ ] **Step 1: 核实** loopback operator 的 `ambient_actor()`：写一条测试打印/断言 Panel 路径 spawn 出来的 `created_by`（`handlers/pty.rs` 的 spawn 测试已有形状）。据 spec §10 的两种情况选实现。
- [ ] **Step 2: 红测试**：`wait_returns_when_the_state_enters_the_until_set`（用 `RuntimeAgents::subscribe` + 独立表实例，另一线程 sample）；`wait_times_out_with_the_current_entry`；`wait_reports_gone_when_the_session_is_removed`；`wait_timeout_is_capped_at_the_tool_budget`（`timeout_ms = 600_000` 被钳到 150_000）；`explain_names_the_matched_rule_and_manifest_version`；`an_actorless_caller_sees_only_unowned_sessions`（三个动词 + `wait`）。
- [ ] **Step 3: 实现**；`DESCRIPTION` 加两句（`wait` 是阻塞观察、`explain` 是解释）；schema 变化会碰 `definitions.rs` 的 DESCRIPTION 上限/残留 pin——按其注释调整而不是抬上限。
- [ ] **Step 4: 证伪**：把零身份臂放回 `owner_admits(_, None) == true`，`an_actorless_caller_sees_only_unowned_sessions` 必红。
- [ ] **Step 5: 绿** `cargo test -p alephcore --lib builtin_tools::terminal executor::builtin_registry gateway::method_authz`。
- [ ] **Step 6: 提交** `builtin_tools/terminal: wait and explain verbs; the actor-less arm sees only unowned sessions`。

### Task E: 真机 QA `qa/terminal/run.sh`

**Files:** `qa/terminal/run.sh` · `qa/terminal/fake-claude`（bash，画 `claude.toml` 规则的 idle/working/blocked 画面，各停 2s；`QUIET=1` 时 working 后停 35s；发 `OSC 7`）· `qa/README.md`（加一段：每个阶段在证明什么）

- [ ] **Step 1**: 读 `qa/agents_viz/run.sh` 与 `qa/lib/*.sh` 的起服/假 key/`tools.invoke` 形状，照抄装置骨架（不要自造第二套）。
- [ ] **Step 2**: 阶段 `identify` / `wait` / `quiet` / `cwd`，全部断言**效果**（`runtime.agents.list` 的字段值），不断言"调用发生了"。
- [ ] **Step 3**: **在本 worktree 构建后跑**，打印 `aleph-server` 路径；四阶段全过才算；任何红先怀疑断言（记忆 `project-marketplace-leftovers-round2`）。
- [ ] **Step 4: 提交** `qa/terminal: identify, wait, quiet and cwd stages against a real gateway`。

### Task F: 文档

- [ ] `docs/reference/FEATURE_LOCATOR.md`：§6.11、§6.12 正文；附录 E.4/E.7 触发器各 2–3 条；附录 D 两条全文（见 spec §4.7）。
- [ ] `docs/reference/TERMINAL_RUNTIME.md` 新建（架构 · 数据流 · 仲裁与常量 · 闸 · wire · herdr 对照 · 刻意不做）。
- [ ] `CLAUDE.md` 路由表：`src/gateway/pty/` 行指向 FL §6.11（现在真的存在）；新增一行 `src/gateway/runtime/` `crates/agent-detect/` `src/builtin_tools/terminal.rs` → `TERMINAL_RUNTIME.md` · FL §6.12 · E.4 · `qa/terminal/run.sh`。只增行。
- [ ] `SECURITY.md` 内嵌终端段、`TOOL_SYSTEM.md` `terminal` 条目、0-A 清单状态、第 1 期 spec 顶部状态行。
- [ ] 提交 `docs: FEATURE_LOCATOR §6.11/§6.12, TERMINAL_RUNTIME.md, routing rows, 0-A statuses`。

---

## 自审（2026-09-04）

1. **spec 覆盖**：§4.1→A1/A2；§4.2→B1–B3；§4.3→C1–C3；§4.4→D；§4.5→Task 0；§4.6→E；§4.7→F；§5 每行都有对应测试名；§7 DECIDE 项无任务（刻意）。
2. **占位符扫描**：无 TBD；每个测试有名字与断言对象；每个任务有验证命令。
3. **类型一致**：`PtySessionInfo`/`PtyListResponse`（Task 0）被 C2 `TabModel::reconcile` 与 D 消费，字段名一致；`RuntimeAgents::subscribe()`（A2）被 D 消费；`Screen::cwd()`（B3）被 M 消费；`state_glyph`/`quiet_label`（C1/C3）被两端消费。
