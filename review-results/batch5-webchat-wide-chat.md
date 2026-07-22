# 静态审查报告：webchat-wide-chat

**审查单元**：`interfaces/webchat/src/platform/wide/views/chat/` + `interfaces/webchat/src/platform/wide/*.rs`  
**关注点**：消息渲染 XSS、文件上传、无限增长的状态  
**审查日期**：2026-07-22  
**代码基线**：`/tmp/aleph-review-batch-5`（与 main 一致）

---

## 1. 统计

| 项目 | 数值 |
|------|------|
| 审查文件数 | 28（chat 目录 27 个 `.rs` + `wide/mod.rs`） |
| chat 目录 LOC | 10,397 |
| `wide/mod.rs` LOC | 9 |
| 发现条数 | 9（Critical 0 / High 1 / Medium 2 / Low 6） |

> 辅助诊断文件 `/tmp/rd-interfaces.json`、`/tmp/rd-desktop.json`、`/tmp/rd-shared.json` 均为空，未作为依据使用。

---

## 2. 发现列表（按严重级排序）

### High

#### H1. 助手消息 Markdown 渲染未过滤危险 URL，可导致点击式 XSS

- **文件**: `interfaces/webchat/src/platform/wide/views/chat/messages.rs:840`、`messages.rs:863`、`messages.rs:964`
- **严重级**: High
- **问题描述**: 助手气泡与中间叙述行均通过 `crate::components::markdown::TypewriterRenderer` 渲染，其底层 `render_markdown` 会把原始 Markdown 事件交给 `pulldown_cmark::html::push_html` 生成 HTML 并写入 `inner_html`。代码虽然对 `Event::Html` / `Event::InlineHtml` 做了 HTML 转义，但 **没有过滤 `<a href>` 或 `<img src>` 中的危险协议**。`pulldown-cmark` 的 HTML renderer 本身不对 URL 做消毒，因此远端/模型返回的内容只要包含 `[text](javascript:alert(1))`，就会被渲染为可执行的 `javascript:` 链接，用户点击即触发 XSS。
- **建议修法**: 在 `render_markdown` 中拦截 `Event::Start(Tag::Link { dest_url, .. })` 与 `Event::Start(Tag::Image { dest_url, .. })`，拒绝或净化 `javascript:`、`data:` 等危险协议；或引入 `ammonia`/`dom_sanitizer` 对最终 HTML 再做一次 URL 白名单消毒。修复后需补充 `[x](javascript:...)` 的回归测试。

---

### Medium

#### M1. 附件上传无大小/数量限制，可造成前端内存膨胀或 OOM

- **文件**: `interfaces/webchat/src/platform/wide/views/chat/composer/attachments.rs:36`、`interfaces/webchat/src/platform/wide/views/chat/view.rs:259`
- **严重级**: Medium
- **问题描述**: `read_file_list_into` 与 `ingest_dropped_file` 直接调用 `FileReader::read_as_data_url`，把整个文件读入内存并提取 base64，期间**未检查 `file.size()`、未限制单个文件大小、未限制附件总数**。用户拖入一个数 GB 的文件即可让 Web 端内存暴涨，甚至导致标签页崩溃；随后完整 base64 载荷会随 `ChatApi::send` 进入网络通道。
- **建议修法**: 在读取前增加大小阈值（如单个文件 ≤10 MB、单次总附件 ≤50 MB）并在 UI 提示；超过阈值的文件直接跳过，不进入 `PendingAttachment` 列表。

#### M2. `trace_runs` 集合只增不减，长会话存在状态无限增长

- **文件**: `interfaces/webchat/src/platform/wide/views/chat/events.rs:506`、`events.rs:609`
- **严重级**: Medium
- **问题描述**: `subscribe_run_events` 用 `Arc<Mutex<HashSet<String>>>` 记录所有产生过 `agent_trace` 事件的 `run_id`（`trace_runs.lock().unwrap().insert(run_id.to_string())`），但代码中**没有任何清理逻辑**。单会话长期运行或频繁切换会话时，该集合会随运行次数线性增长。
- **建议修法**: 在 `run_complete` / `run_error` 处理分支中，从 `trace_runs` 移除已结束的 `run_id`；或改用 LRU/会话切换时清空。

---

### Low

#### L1. 生产代码使用 `expect`，违反项目错误处理规范

- **文件**: `interfaces/webchat/src/platform/wide/views/chat/todo_panel.rs:33`
- **严重级**: Low
- **问题描述**: `Show when=visible` 的子闭包中写了 `chat.plan.get().expect("visible implies Some")`。虽然当前逻辑保证 `visible` 为真时一定有值，但 `expect` 出现在非测试代码中，违反 AGENTS.md「生产代码禁止 `unwrap()`/`expect()`」的红线。
- **建议修法**: 改为 `if let Some(plan) = chat.plan.get() { ... } else { return view! {}; }`。

#### L2. 多处 `Closure::forget()` 泄漏 JS 闭包与 DOM 句柄

- **文件**: `interfaces/webchat/src/platform/wide/views/chat/composer/attachments.rs:78`、`interfaces/webchat/src/platform/wide/views/chat/view.rs:288`、`interfaces/webchat/src/platform/wide/views/chat/composer/voice.rs:187`
- **严重级**: Low
- **问题描述**: 文件读取、拖放、语音录制等一次性回调通过 `Closure::wrap(...).forget()` 泄漏到 JS 堆，Rust 侧无法回收。单次泄漏很小，但在大量文件操作或长时间会话下会累积。
- **建议修法**: 将闭包保存在组件状态或句柄结构体中，在任务完成/组件卸载时显式 `drop`；拖放/附件读取完成后可立即释放。

#### L3. `InputArea` 的 `ResizeObserver` 未断开且闭牌泄漏

- **文件**: `interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs:92-112`
- **严重级**: Low
- **问题描述**: `Effect` 内创建 `ResizeObserver` 后调用 `cb.forget()`，且没有 `disconnect()` 调用。注释称“one-per-app”，但代码未在组件卸载时清理 observer，仍属于可避免的句柄泄漏。
- **建议修法**: 保存 observer 与 closure 句柄，在 `on_cleanup` 中调用 `observer.disconnect()` 并 drop 闭牌。

#### L4. 拖放附件逻辑与 paperclip 附件逻辑重复

- **文件**: `interfaces/webchat/src/platform/wide/views/chat/view.rs:259-289`、`interfaces/webchat/src/platform/wide/views/chat/composer/attachments.rs:36-82`
- **严重级**: Low
- **问题描述**: `ingest_dropped_file` 与 `read_file_list_into` 的 base64 读取、mime 回退、附件 push 逻辑几乎一致。重复代码容易让后续的大小限制、安全校验只加在一处而漏掉另一处（如 M1 目前同时影响两条路径）。
- **建议修法**: 把 `ingest_dropped_file` 实现收敛到 `attachments.rs` 的公共读取函数，两个入口只负责收集 `web_sys::File`。

#### L5. 多个文件超过 500 行，影响可维护性

- **文件**: 
  - `interfaces/webchat/src/platform/wide/views/chat/state.rs`（1,728 行）
  - `interfaces/webchat/src/platform/wide/views/chat/composer/mod.rs`（1,202 行）
  - `interfaces/webchat/src/platform/wide/views/chat/events.rs`（1,191 行）
  - `interfaces/webchat/src/platform/wide/views/chat/messages.rs`（1,139 行）
  - `interfaces/webchat/src/platform/wide/views/chat/timeline.rs`（821 行）
- **严重级**: Low
- **问题描述**: 超过 AGENTS.md 建议的 500 行上限，尤其 `state.rs` 接近 1,800 行，`composer/mod.rs` 虽然拆分出子模块但 orchestrator 本身仍超 1,200 行。
- **建议修法**: 对 `state.rs` 可进一步拆分为 `chat_state.rs`/`snapshot.rs`/`send_error.rs`；`composer/mod.rs` 的视图/键盘/Effects 块可继续拆分到 `composer/input.rs` 等子模块。

#### L6. `messages.rs` 中 `chat.messages.get()` 在 Memo/Effect 中多次全量克隆

- **文件**: `interfaces/webchat/src/platform/wide/views/chat/messages.rs:115`、`messages.rs:125`、`messages.rs:163`
- **严重级**: Low
- **问题描述**: `MessageList` 的时间轴 Memo、归因 Memo、滚动 Effect 都通过 `chat.messages.get()` 读取并克隆整个消息向量。消息历史很长时，每次更新都会触发 O(n) 克隆，存在性能退化风险（与“无限增长的状态”关注点相关）。
- **建议修法**: 尽量使用 `.with(|msgs| ...)` 只读访问；时间轴与归因计算若只需 id/长度等派生信息，可在读取时避免完整 `Vec<ChatMessage>` 克隆。

---

## 3. 架构红线合规快照

| 红线 | 结论 | 说明 |
|------|------|------|
| R1 core 不调用平台 API | ✅ 不适用 | 本单元为 Web 接口层，调用 `web_sys`/`leptos` 属于接口层职责，未出现 core 层越权。 |
| R2 复杂业务 UI 在 Leptos/WASM | ✅ 合规 | 聊天视图完全由 Leptos/WASM 组件构成。 |
| R3 core 极简，非核心功能不引入重依赖 | ✅ 合规 | webchat 接口层引入了 `pulldown-cmark`、`syntect` 等，但这是界面渲染所需，未违反 core 层约束。 |
| R4 接口层为纯 I/O，无业务逻辑 | ⚠️ 基本合规 | composer 中有少量客户端决策（prompt-injection  guard、队列 drain、tier/mode 透传），整体仍属 I/O 编排，未出现核心编排逻辑下沉。 |
| R7 Rust Core 是唯一大脑 | ✅ 合规 | 聊天状态投影、工具调用、计费、上下文占用均来自 core 事件或 RPC。 |
| R8 LLM 负责意图/路由，正则只用于机器格式 | ✅ 合规 | chat 单元未发现用正则做用户输入意图解析。 |
| R9 所有可配置项暴露为工具 | ✅ 合规 | tier/mode 均通过 core 的 `sessions.patch` / `chat.send` 配置，符合“工具驱动”。 |
| R10 智能在 prompt 中 | ✅ 合规 | doctor repair prompt 等智能表达放在常量 prompt 中，无中间件包装。 |

---

## 4. 修复优先级建议

1. **High**: 优先处理 Markdown 链接 URL 消毒（H1），这是本单元唯一可导致代码执行的问题。
2. **Medium**: 为附件上传增加大小/数量上限（M1），并与拖放路径统一收敛（L4）；清理 `trace_runs`（M2）。
3. **Low**: 替换 `todo_panel.rs` 的 `expect`（L1），回收泄漏的 `Closure`/`ResizeObserver`（L2、L3），并按 500 行目标逐步拆分超大文件（L5）。
