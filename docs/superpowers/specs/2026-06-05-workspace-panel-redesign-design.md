# 工作区面板重设计 (Workspace Panel Redesign)

**日期**: 2026-06-05
**范围**: `interfaces/webchat`(Leptos Panel)+ `src/gateway/handlers/fs.rs`(Core 新增一个 RPC)

## 1. 背景与问题 (Background)

Panel 右侧的"工作区面板"(workspace pane)当前几乎永远是空的。审查后确认这是**定位问题,不是 bug**:

- 物理形态:右侧分栏(`LayoutMode::Split`,聊天 33% / 工作区 66%),由顶部 toggle 控制挂载。
- 状态模型只有两个内容变体:`WorkspaceContent::Empty` 与 `WorkspaceContent::ToolDetail { run_id, tool_id }`。
- 唯一填充路径:用户必须在聊天气泡里**手动点击一个工具调用 chip**(`views/chat/messages.rs:307` → `WorkspaceState::show_tool`)。
- 触发条件三者缺一不可:① 开了 Split;② 当前会话**真的调用了工具**;③ 用户**主动点** chip。任一不满足就停在 `Empty` 的 hero 占位图。

代码注释自称 "UI-TARS-parity primitive",但实际只是一个被动的工具调用 JSON 查看器,与 UI-TARS-desktop / DeepSeek-Reasonix 那种"看得见 Agent 在干活"的工作台存在巨大落差。

## 2. 目标 (Goal)

把这块区域从"点了才看的工具 JSON 查看器"改成:

> **自动滚动的 Agent 执行活动流(主)+ 项目文件树预览(辅助抽屉)**

方向已与用户确认:**B(自动活动流)为核心 + C 的文件预览**,内部排布按 **C(活动流为主 + 底部文件树抽屉)**。

## 3. 架构红线约束 (Redlines)

| 红线 | 本设计如何满足 |
|------|----------------|
| **R4** Interface 纯 I/O | 活动流 100% 派生自已有的 `run.*` 事件数据;文件预览靠 Core 新增 `fs.read_file` RPC(纯 I/O)。Panel 不做任何业务计算。 |
| **R2** UI 逻辑唯一源 | 全部 UI 在 Leptos Panel 实现,原生 Bridge 不参与。 |
| **R5** AI 主动到达 / 不打扰 | 用户在 ChatOnly 时**不强行弹开分栏**;仅在 toggle 按钮上加活动指示徽标,用户自行点开。已开 Split 时才自动滚动填充。 |
| **R7** LLM 主权 | 不引入任何推理/分类逻辑。edit 类工具直接用 args 里的 `old/new` 并排展示,**不需要 diff 算法**。 |

## 4. 数据流 (Data Flow)

```
run.* 事件 (已存在)
  └─ events.rs → ChatState.messages[].tool_calls   ← timeline 顺序来源
              └─ WorkspaceState.tool_payloads       ← 每个工具的 args/result

活动流 = 对上面两者的反应式只读视图(零新增持久状态,随事件自动更新)

文件树抽屉:
  fs.allowed_roots / fs.list_dir (已存在) → 文件树,根 = chat.active_project_root
  fs.read_file (★ 新增 Core RPC)          → 选中文件的内容预览
```

关键洞察:**活动流几乎是对现有 `ChatState` + `tool_payloads` 的纯渲染**,无需新增数据通道。这让 Phase 1 完全不动 Core。

## 5. 组件与状态改动 (Components & State)

### 5.1 Panel 侧

**`state/layout.rs` — `WorkspaceState`**

- 废弃 `WorkspaceContent`(`Empty | ToolDetail`)的单视图语义。工作区改为持久多区视图。
- `WorkspaceState` 新增字段:
  - `files_drawer_open: RwSignal<bool>` — 底部文件树抽屉开合,默认 `false`。
  - `selected_file: RwSignal<Option<FilePreview>>` — 当前预览文件(path + content),懒加载。
  - `expanded_events: RwSignal<HashSet<String>>` — 活动流里就地展开的 `tool_id` 集合。
- 新增类型 `FilePreview { path: String, content: String, truncated: bool }`。
- `reset()` 语义扩展:会话切换时同时清空 `files_drawer_open`(回 false)、`selected_file`、`expanded_events`(沿用现有"会话作用域"清理)。
- `mode`(Split/ChatOnly)仍持久化在 localStorage,跨会话保留(不变)。

**`components/workspace_panel.rs` — 重写**

- `ActivityTimeline`:派生自 `ChatState.messages`,把所有 assistant 消息的 `tool_calls` 按文档顺序展平为时间线行。每行显示工具名 / 状态 / 耗时。文件操作类工具(read_file / write_file / edit 等)的行可就地展开:
  - `edit`:用 args 里的 `old_string` / `new_string` 并排展示(无 diff 算法)。
  - `write_file`:展示新内容。
  - `read_file`:展示 result 内容。
  - 其它工具:展开显示 args / result JSON(沿用现有 `JsonViewer`)。
- `FilesDrawer`:底部可展开抽屉。展开后显示文件树(复用 `fs.list_dir`,根 = `chat.active_project_root`,无 project 时回退 `fs.allowed_roots`)+ 选中文件预览(`fs.read_file`)。
- toggle 按钮(`components/layout_toggle.rs`)加活动指示徽标:有新工具活动且面板未打开时脉冲/计数。

**`views/chat/messages.rs`**

- 旧的点 chip → `show_tool` 路径保留,但语义改为"高亮 / 滚动到 timeline 对应行 + 展开它",不再是工作区唯一入口。

### 5.2 Core 侧

**`src/gateway/handlers/fs.rs` — 新增 `fs.read_file`**

- 入参 `{ path }`,返回 `{ path, content, truncated }`。
- 复用 `fs.list_dir` 同款 `projects.allowed_roots` 安全校验(越界拒绝、符号链接 canonicalize)。
- 大小上限:超过阈值截断并置 `truncated = true`(对齐 `list_dir` 的 `MAX_ENTRIES` 风格常量)。
- 在 `bin/aleph-server/.../handlers/settings.rs` 注册 `register_handler!(server, "fs.read_file", ...)`。
- Panel 侧 `api/fs.rs` 加 `FsApi::read_file` 客户端方法。

## 6. 构建顺序 (Build Sequence — 可垂直切片)

1. **Phase 1(零 Core 改动)**:重写 `WorkspaceState` / `workspace_panel.rs`,渲染自动活动流 + 文件操作行就地展开。**交付 B + "碰过的文件预览"**。
2. **Phase 2(Core + Panel)**:加 `fs.read_file` RPC + `FilesDrawer`(文件树 + 预览)。**交付 C 的"随时浏览任意文件"**。
3. **贯穿**:toggle 活动徽标 + R5 不抢焦点行为。

## 7. 测试 (Testing)

**Panel(`#[cfg(test)]`,沿用现有 `Owner::new()` 模式)**
- 活动流从 `ChatState.messages` 派生的顺序正确(多消息、多工具)。
- 文件操作行展开渲染(edit 并排 / write 内容 / read 内容)。
- `files_drawer_open` 开合;`reset()` 清空新增的三个字段但保留 `mode`。
- `FsApi::read_file` 响应解析(含 `truncated`)。

**Core(`src/gateway/handlers/fs.rs` tests)**
- `fs.read_file` 越界路径拒绝。
- 大文件截断(`truncated = true`)。
- 符号链接逃逸拒绝(对齐 `list_dir` 现有测试)。

## 8. 已确认的设计决策 (Confirmed Decisions)

1. **diff 就地看**:文件操作的内容/diff 展开在**活动流行内**(贴合 C);文件树预览只显示文件当前完整内容。
2. **R5 不抢焦点**:ChatOnly 时不自动弹分栏,只在 toggle 上加活动徽标。
3. **timeline 作用域**:仅当前会话;会话切换由扩展后的 `reset()` 清理。
4. **文件树根**:`chat.active_project_root`,无 project 时回退 `fs.allowed_roots`。

## 9. 非目标 (Non-Goals)

- 不做多模态产物预览(图片/视频画廊)、Plan/TODO、推理 trace 等完整 C 形态——本轮只做活动流 + 文件预览。
- 不做文件编辑/写入(预览只读)。
- 不引入 diff 算法或任何内容分类/推理逻辑(R7)。
