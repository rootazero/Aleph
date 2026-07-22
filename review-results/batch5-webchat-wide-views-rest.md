# 静态审查报告：webchat-wide-views-rest

## 审查范围

- **单元名**：webchat-wide-views-rest
- **路径**：`interfaces/webchat/src/platform/wide/views/{canvas,voice,teams,cron,agents,extensions,memory,memory_hub,subagent_tree}/`
- **关注点**：voice（麦克风权限与音频数据）、canvas（渲染注入）、其余 wide 视图
- **统计**：67 个 `.rs` 文件，约 18,676 行代码（含注释与测试）

## 总体结论

- **Critical**：0
- **High**：1
- **Medium**：7
- **Low**：12

未发现 XSS/渲染注入、生产代码 `unwrap()`/`expect()`、凭据泄漏、未授权 SSRF 等严重安全缺陷。voice 与 canvas 的注入面均处于受控状态。主要风险集中在 **异步响应竞态覆盖** 与 **少量业务逻辑下沉到接口层**。

---

## 发现问题（按严重级排序）

### High

| 文件:行号 | 问题描述 | 建议修法 |
|-----------|----------|----------|
| `interfaces/webchat/src/platform/wide/views/agents/files.rs:61` | 文件加载失败时把 `"Error loading file: {e}"` 直接写入 `file_content`，而保存按钮（`files.rs:187`）会原样回写到服务端，可能把错误文本覆盖进 `SOUL.md` 等身份文件。 | 加载失败时单独设置错误消息或清空 `selected_file`/`file_content`，禁止污染保存数据源。 |

### Medium

| 文件:行号 | 问题描述 | 建议修法 |
|-----------|----------|----------|
| `interfaces/webchat/src/platform/wide/views/teams/replay.rs:60-66` | `refresh_trace` 的 `spawn_local` 完成后无条件 `trace.set(...)`；快速切换任务时，旧慢响应会覆盖当前选中任务的 trace。 | spawn 前捕获 `selected.get()` 的 `task_id`，await 返回后校验仍等于当前选中再写入。 |
| `interfaces/webchat/src/platform/wide/views/teams/components/task_drawer.rs:108-122` | 切换抽屉任务时并发发起 `list_task_runs/comments/events`，三个响应均无条件写信号；旧任务的响应可能落在清空后的新任务抽屉中。 | 每个 spawn 捕获任务 id，返回后与 `open_for` 当前 id 比对，不匹配则丢弃。 |
| `interfaces/webchat/src/platform/wide/views/cron/job_editor.rs:164-170` | 选择 job 后并发加载 run-history，无归属校验；快速切换 A→B 时 A 的慢响应会覆盖 `runs`。 | await 后校验当前选中 `job_id` 是否仍为发起时的 id。 |
| `interfaces/webchat/src/platform/wide/views/agents/mod.rs:89-113` | `Effect` 依赖 `agent_id`，URL 快速变化时多个 `AgentsApi::list` 并发在飞，旧响应会把 `agent_summary` 设回前一个 agent，且失败路径只打 console。 | 响应落地前比对发起时的 `id` 与当前 `agent_id.get()`；失败时更新错误信号。 |
| `interfaces/webchat/src/platform/wide/views/memory/mod.rs:102-119` | notes 加载在 agent 切换时无代数/当前 agent 校验，慢的旧 agent 响应会覆盖 `notes_window`。 | 携带发起时的 `agent` 快照，返回后比对 `mem.agent_id` 是否一致。 |
| `interfaces/webchat/src/platform/wide/views/memory/mod.rs:122-144` | raw 搜索/翻页同样无竞态防护，乱序完成会使陈旧页结果覆盖新结果。 | 携带 `(page, query, agent)` 快照，返回后三者均匹配再写入。 |
| `interfaces/webchat/src/platform/wide/views/subagent_tree/mod.rs:116` | `state.subscribe_events(...)` 返回的订阅 id 被丢弃，Effect 每次在重连时都会重复注册 handler，组件卸载也不退订；事件会被重复应用 N 次。 | 保存 `sub_id` 并在 `on_cleanup` 中调用 `state.unsubscribe_events(sub_id)`。 |

### Low

| 文件:行号 | 问题描述 | 建议修法 |
|-----------|----------|----------|
| `interfaces/webchat/src/platform/wide/views/agents/files.rs:40` | API 错误直接 `console.error_1(&format!("Failed to list files: {e}"))`，错误串可能包含服务端路径等敏感信息。 | 仅输出用户级提示；调试信息使用 `tracing` 或受控日志级别。 |
| `interfaces/webchat/src/platform/wide/views/cron/job_list.rs:111-113` | Quick Create 使用 `let _ = CronApi::create(...).await;` 静默吞错，且成功后不本地刷新，完全依赖事件推送。 | 处理 `Err` 分支并设置错误提示；成功后主动刷新列表或等待推送时给出加载反馈。 |
| `interfaces/webchat/src/platform/wide/views/cron/job_editor.rs:385-386` | `run_now` 成功后 `sleep(3s)` 再清空 `run_success`；连续点击两次时第一次的定时器会提前清掉第二次的成功提示。 | 使用递增代际号，仅当当前代际仍最新时才清空提示。 |
| `interfaces/webchat/src/platform/wide/views/teams/overview.rs:123,147` | 解散/删除失败提示使用硬编码中文 `"解散失败: {e}"` / `"删除失败: {e}"`，绕过了文件内统一使用的 i18n。 | 补 i18n key，使用 `t_string!` 渲染。 |
| `interfaces/webchat/src/platform/wide/views/teams/plan_dag.rs:94` | 空态文案 `"No tasks yet for this team."` 硬编码英文，未走 i18n。 | 引入 `use_i18n` 并补 key。 |
| `interfaces/webchat/src/platform/wide/views/agents/overview.rs:195,216` | `<option>` 与失效 model 警告使用硬编码中英文字符串，与周边 `t!(i18n, ...)` 用法不一致。 | 迁移到 i18n 资源。 |
| `interfaces/webchat/src/platform/wide/views/memory/drawer.rs:263,313,327-329` | 按钮文案 `"Confirm"`、`"Rename"`、`"Delete"`、`"Confirm delete?"` 为硬编码英文。 | 补充对应 i18n key。 |
| `interfaces/webchat/src/platform/wide/views/memory/drawer.rs:388-403` | `navigate_drawer` 在 `graph.search` await 完成后无条件设置 `target_signal`；若用户已关闭 drawer，响应到达后会重新打开。 | await 后检查 drawer 当前是否仍打开，或仅当 `target_signal` 仍与发起时一致才写入。 |
| `interfaces/webchat/src/platform/wide/views/memory/mod.rs:347` | notes 分支给 `Pager` 传入 `current_len=Signal::derive(|| 0usize)` 常量占位，而 `notes_total` 恒为 `Some`，该参数在此分支无效。 | 删除该无效参数或统一 `Pager` 语义。 |
| `interfaces/webchat/src/platform/wide/views/teams/components/task_drawer.rs:48-59` | `actions_for_status` 把任务生命周期状态机迁移规则硬编码在接口层，core 改规则时此处会静默漂移。 | 由后端 DTO 下发 `available_actions`，视图只负责渲染。 |
| `interfaces/webchat/src/platform/wide/views/teams/components/board.rs:20-29` | "unsatisfiable 折叠进 Blocked" 的派生语义由视图层判定，与 `task_drawer.rs:52` 的 `"blocked" | "unsatisfiable"` 分支重复实现同一业务规则。 | 收敛到 core 下发的规范化状态或共享 helper。 |
| `interfaces/webchat/src/platform/wide/views/cron/helpers.rs:195-235` | `build_schedule_kind_json`/`extract_schedule_from_kind` 在接口层内嵌后端 `schedule_kind` tagged-enum 的序列化契约（含 `delete_after_run: true` 等业务默认值）。 | 将 schedule_kind 构造/解析下沉到 `shared/protocol` 或 core 类型。 |
| `interfaces/webchat/src/platform/wide/views/cron/job_editor.rs:1-840` | 单文件 840 行，单组件持有约 20 个 `RwSignal` 表单字段，超过 500 行阈值。 | 拆分表单 section 子组件或引入结构化 form-state。 |
| `interfaces/webchat/src/platform/wide/views/memory/mod.rs:1-695` | 单文件 695 行，含 `Memory/Pager/NotesTable/RawTable/RawRow` 五个组件。 | 将表格组件拆到独立模块。 |

---

## 安全专项说明

- **XSS / 渲染注入**：
  - `canvas/node_detail_panel.rs:386` 与 `memory/drawer.rs:284` 均使用 `inner_html=`，但渲染的是 `crate::canvas_engine::markdown_excerpt::render_excerpt`，该函数对 raw HTML 进行转义、对链接 scheme 做白名单，确认安全。
  - `canvas/node_detail_panel.rs:289` 的 `style` 拼接来源 `category_color()`，只返回固定 `var(--cat-*)` 或 `hsl(hue,55%,65%)`，无法注入 CSS payload。
  - canvas WebGL 着色器为静态字符串常量，无外部字符串拼接注入点。
- **麦克风权限与音频数据（voice）**：
  - `voice/audio.rs` 对 `getUserMedia` 拒绝做了按 DOMException name 的细分映射（Denied/NotFound/NotReadable/Unsupported/AudioContext/Other），不再统一甩锅“权限”。
  - `MicError::settings_url` 仅在 `native` shell 且 `Denied` 时生成 macOS 系统偏好设置 deep-link；浏览器场景返回 `None`，避免误导用户。
  - 连续 PCM tap、pre-roll、segment 上限、VAD/echo-aware barge-in 逻辑均位于正确位置。
- **生产代码 `unwrap()`/`expect()`**：本单元生产代码无 `unwrap()`/`expect()`/`panic!`（仅在 `#[cfg(test)]` 块中出现，符合规范）。
- **SSRF / 任意 URL**：本单元所有网络请求均通过封装 API（`ChatApi`、`TeamsApi`、`CronApi`、`AgentsApi`、`MemoryApi`、`GraphApi`）发起，无用户输入直接构造 URL 的 `fetch`/`window.open`。
- **凭据泄漏**：未发现密码/token 写入日志或 UI；API 错误串可能含路径，已在 Low 项中提示。

---

## 架构红线合规快照

| 红线 | 状态 | 说明 |
|------|------|------|
| R1 core 不调用平台 API | ✅ 合规 | 本单元为 webchat 视图层，直接使用 `web_sys` 属于接口层职责。 |
| R2 复杂业务 UI 在 Leptos/WASM | ✅ 合规 | 所有复杂视图均为 Leptos 组件。 |
| R3 core 极简，非核心不重依赖 | ✅ 合规 | 本单元为 interface 层，不评估 core。 |
| R4 接口层纯 I/O，无业务逻辑 | ⚠️ 部分越界 | `actions_for_status`、`board` 状态分组、`cron/helpers.rs` schedule_kind 序列化属于接口层承载的业务规则，已在 Low 项中列出。 |
| R7 Rust Core 是唯一大脑 | ✅ 合规 | 视图仅展示/转发，状态变更均通过 JSON-RPC 到 core。 |
| R8 正则只用于机器格式 | ✅ 合规 | 本单元未使用 `regex`。 |
| R9 所有可配置项暴露为工具 | ✅ 未涉及 | 视图本身不暴露配置入口。 |
| R10 智能在 prompt 中 | ✅ 未涉及 | 视图无 LLM 中间件逻辑。 |

---

## 审查方法

- 对 voice、canvas 关键文件逐行阅读；对其余目录使用并行子代理扫描后，对每条候选问题回到源码位置二次确认，排除猜测项。
- 未运行 `cargo check/build/clippy`，未修改源码，未执行任何 git 操作。
