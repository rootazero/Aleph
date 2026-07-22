# 静态审查报告 — webchat-components

- 单元：`webchat-components` | `interfaces/webchat/src/components/`
- 关注点：markdown.rs 的 XSS/inner_html、表单注入、不受信内容渲染
- 审查日期：2026-07-22（基于 /tmp/aleph-review-batch-5 worktree，与 main 一致）
- rust-doctor 辅助：`/tmp/rd-interfaces.json` 为空文件，无可用线索，全部为人工阅读确认

## 统计

- Rust 源文件：63 个（含 extensions/inspector/layouts/sidebar/ui 子目录）
- 总 LOC：11,482（另有 `forms_examples.md` 示例文档 1 个，不参与编译）
- 超大文件（>500 行）：`chat_sidebar.rs` 1771 行、`tool_card.rs` 1134 行

## 发现列表（按严重级排序）

### High

**1. `markdown.rs:56-77` — Markdown 链接 URL 未做 scheme 过滤，`javascript:` 链接可 XSS（需用户点击）**

`render_markdown` 只转义了 `Event::Html` / `Event::InlineHtml`（raw HTML 注入已堵住），并转义了 code fence 的 info-string。但 `Tag::Link` / `Tag::Image` 事件走 `other` 分支交给 `pulldown_cmark::html::push_html`（markdown.rs:72），pulldown-cmark 不过滤危险 URI scheme。assistant/远端不可信内容中的 `[点我](javascript:alert(document.cookie))` 会被渲染成 `<a href="javascript:...">`，经 `inner_html`（markdown.rs:238/348/360）注入 DOM，用户点击即在面板源内执行任意 JS（可读取 WebSocket 令牌、发 RPC）。

佐证：同仓库 `canvas_engine/markdown_excerpt.rs:141` 已有 `sanitize_link_url`（白名单 http/https/mailto，其余改写为 `#disallowed-*`），注释明确"Reject `javascript:` … to prevent XSS when the excerpt is assigned to innerHTML"——同一风险在 canvas 摘录渲染中已修，主聊天气泡渲染器漏修。

修法：在 `render_markdown` 的事件循环中拦截 `Event::Start(Tag::Link { dest_url, .. })`，对 `dest_url` 应用与 `sanitize_link_url` 相同的白名单（建议把该函数移到共享位置复用）；`Tag::Image` 的 URL 同理过滤（顺带挡 `data:` 钓鱼）。补一条 `[x](javascript:...)` 的回归测试（现有测试只覆盖 fence info-string）。

### Medium

**2. `chat_sidebar.rs:364,388,397` — 异步续体未防护组件销毁后的信号访问，dispose 竞态可 panic**

`reload_data` 的 `spawn_local` 任务里，作者已意识到 cold-start 销毁竞态，对 `selected_agent` 用了 `try_get_untracked` 提前返回（chat_sidebar.rs:332）。但后续多个 await 点之后仍有未防护的 `sessions.set(list)`（约 364 行）、`groups.set(team_list)`（约 388 行）、`is_loading.set(false)`（约 397 行）。组件在 `sessions.list` / `run_concurrency` / `agent_teams` 任一 await 期间被卸载时，这些 `set` 访问已 dispose 的 `RwSignal`，debug 构建直接 panic（release 为告警 no-op）。该组件持有大量局部信号（280-312 行），侧边栏切换/重挂载是常态。

修法：对这些信号统一用 `try_set`/`try_update`（或 try_get 后早退），与 332 行的既有处理保持一致。

### Low

**3. `extensions/detail_drawer.rs:49` / `extensions/trust_modal.rs:29` — `<Show>` 子闭包内 `.unwrap()` 信号**

`store.selected.get().unwrap()` / `store.disclosure.get().unwrap()` 依赖外层 `when=is_some` 守卫。Leptos 中 `when` 与子闭包各自 track 同一信号，信号变 None 时两者的重跑顺序无强保证，存在理论上的 panic 窗口；同时违反生产代码禁 `unwrap()` 的代码风格。建议改用 `let Some(x) = ...get() else { return ().into_any() }`。

**4. `team_participants.rs:121` — `cb.forget()` 每次 effect 重跑泄漏一个 `Closure`**

ResizeObserver 本身在重跑前/清理时正确 disconnect（88-101、124-137 行），但回调 `Closure` 用 `forget()` 永久泄漏。effect 每重跑一次（NodeRef 变化）就泄漏一份闭包。建议把 `Closure` 一并存入 `observer_store`（如 `StoredValue<Option<(ResizeObserver, Closure<...>)>>`），在 disconnect 的同时 drop。

**5. 质量：`chat_sidebar.rs`（1771 行）、`tool_card.rs`（1134 行）超过 500 行阈值**

会话行/群聊行/代理下拉/搜索/重命名/删除确认等多套状态机与视图混在一个组件文件内（chat_sidebar.rs:280-312 仅信号声明就 30+ 个）。建议按子块（session row、group row、agent picker）拆为子组件。`tool_card.rs` 已把纯逻辑与视图分离且逻辑可宿主机测试，属可接受但仍偏长。

## 已确认无问题（高置信度）

- **raw HTML 注入**：markdown.rs 对 `Event::Html`/`InlineHtml` 统一 `html_escape`，流式渲染器 `render_streaming` 全文转义（markdown.rs:211/215），fence info-string 两处均转义（47-54、198-204），并有回归测试。
- **表单注入**：`forms.rs`、`json_schema_form.rs`、`ask_user_card.rs` 全部走 Leptos 属性/文本绑定（自动转义），无 inner_html 拼接用户输入；`SelectInput`/`TextInput` 的 `&'static str` 约束迫使动态值走原生元素绑定而非拼接。
- **inner_html 其余使用点**：`channel_card.rs:50`、`nav_menu.rs:115/162`、`mode_sidebar.rs:302` 均为 `&'static str` 图标 SVG，无外部输入。
- **密钥处理**：`secret_input.rs`（password/text 切换）、`provider_key_field.rs`（不回显已存密钥，空值=保留）实现谨慎，无明文泄漏路径。
- **生产 unwrap/expect**：除上述 Low 项外，其余 `unwrap` 均在 `#[cfg(test)]` 或 `unwrap_or*` 形式；`expect_context` 为 Leptos 标准惯用法。
- **detail_drawer.rs:122** 外链 `href=url` 带 `rel="noopener"`，且 Leptos 属性绑定会转义属性值。

## 架构红线合规快照

| 红线 | 状态 | 说明 |
|------|------|------|
| R1 | 合规 | 无平台 API 调用；web_sys 仅用于 DOM/主题检测（前端本职） |
| R2 | 合规 | 复杂业务 UI 全在 Leptos/WASM |
| R4 | 合规 | 组件为纯 I/O：渲染 + RPC 转发（`dash.rpc_call`、各 `*Api`），意图解释明确交给 core（ask_user_card.rs:9-16 注释） |
| R7 | 合规 | 无本地业务逻辑副本；澄清/审批均单 RPC 解析、以服务端事件为准 |
| R8 | 合规 | `ToolKind::from_name` 的工具名匹配属机器标识符分类，非自然语言意图识别 |
| R1/R3 依赖 | 合规 | pulldown-cmark/syntect/similar 均为前端渲染必需，未引入重依赖 |

## 结论

主渲染管线的 raw-HTML 注入面已收敛得较好，唯一实质安全缺口是 **链接/图片 URL 的 scheme 白名单缺失（High-1）**——且仓库内已有现成修法可复用。其余为 1 个 dispose 竞态（Medium）与若干 Low 质量问题。无 Critical。
